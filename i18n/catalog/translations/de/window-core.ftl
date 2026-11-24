journal.setup.cancel.admission = Installationszulassung nicht verfügbar

journal.setup.cancel.attempt = Installationsversuch nicht verfügbar

journal.setup.cancel.busy = das Installationsprogramm hält seine Sperre noch

journal.setup.cancel.changed = Installationsversuch geändert

journal.setup.cancel.failed = das Abbrechen ist nicht gelungen

journal.setup.cancel.identity = Identität des Installationsprogramms nicht verfügbar

journal.setup.cancel.owner = Eigentümer des Installationsprogramms nicht verfügbar

journal.setup.cleanup.legacy.log = Bereinigung des alten Protokoll-Alias fehlgeschlagen: { $error }

journal.setup.cleanup.retired.support = Zustandsbereinigung des stillgelegten Support-Portals ist nicht gelungen: { $error }

journal.setup.doctor.blocked = doctor-Blocker fehlgeschlagen

journal.setup.doctor.failed.status = doctor abgeschlossen mit Status { $status }

journal.setup.doctor.incomplete = der JSONL-Datenstrom von doctor endete ohne { $event }

journal.setup.doctor.silent = doctor meldete { $seconds } s lang keinen Fortschritt

journal.setup.doctor.timeout = doctor hat nach { $seconds } s die Zeit überschritten

journal.setup.doctor.unreadable = das JSON von doctor ließ sich nicht parsen

journal.setup.downloads.cancel = abbrechen

journal.setup.downloads.cancelled = abgebrochen

journal.setup.downloads.complete = bereit

journal.setup.downloads.error = Download ist nicht gelungen. versuch es noch einmal.

journal.setup.downloads.idle = nicht gestartet

journal.setup.downloads.progress = wird heruntergeladen…

journal.setup.downloads.retry = erneut versuchen

journal.setup.downloads.speech = Spracherkennung

journal.setup.downloads.thinking = lokales Denken

journal.setup.drive.unsupported = dein Journal kann auf diesem Laufwerk nicht laufen, weil das Laufwerk Dateien nicht so speichern kann, wie dein Journal es braucht. verwende ein Mac-formatiertes Laufwerk für dein Journal.

journal.setup.fits.app = läuft auf jedem Gerät, das du wählst, und nimmt entgegen, was du mit ihr teilst. alles davon geht in dein Journal.

journal.setup.fits.app.term = die { $platform_name }-App

journal.setup.fits.journal.computer = lebt auf dem Computer deines Journals und behält alles, was in sie eingeht.

journal.setup.fits.journal.phone = lebt auf diesem Telefon und behält alles, was hineingeht.

journal.setup.fits.thinking = hilft deinem Journal, zu verstehen, was es behält. wähle unten, wo es geschieht, oder später.

journal.setup.fits.thinking.term = Denken

journal.setup.fits.title = wie es zusammenpasst

journal.setup.folder.change = Ordner ändern

journal.setup.folder.continue = fortfahren

journal.setup.folder.help = wo dein Journal auf dem Computer deines Journals aufbewahrt wird.

journal.setup.folder.title = Journalordner

journal.setup.footer.computer = dein Journal wird auf dem Computer deines Journals behalten. Denken nutzt die Option, die du wählst.

journal.setup.footer.phone = dein Journal wird auf diesem Telefon behalten. Denken nutzt die Option, die du wählst.

journal.setup.heading = dein Journal

journal.setup.help.text = hängst du fest, oder hast du eine Frage?

journal.setup.identity.ca.invalid = ungültige lokale CA: { $detail }

journal.setup.identity.ca.randomness.unavailable = System-CSPRNG nicht verfügbar

journal.setup.identity.foreign = diese Installation ließ sich nicht überprüfen.{ $paragraph }Details: { $error }{ $paragraph }die Befehlszeilenwerkzeuge oder der Hintergrund-Support auf diesem Computer wurden von einer anderen { $platform }-Installation eingerichtet, daher wurde nichts geändert.

journal.setup.identity.invalid = ungültiger Zustand des lokalen Links: { $detail }

journal.setup.identity.io.error = { $path }: { $error }

journal.setup.identity.locations = diese Installation konnte nicht überprüft werden.{ $paragraph }Details: { $error }{ $paragraph }Wiederherstellungsorte der Ersteinrichtung sind nicht verfügbar: { $location_error }

journal.setup.identity.lock.timeout = Sperre für { $path } konnte nicht innerhalb von { $seconds } s erlangt werden

journal.setup.identity.mark.asset.invalid = ungültiges mark-Asset: { $detail }

journal.setup.identity.mark.jid.invalid = ungültige Journal-ID: { $detail }

journal.setup.identity.no.candidate = kein vorbereiteter Link-Identitätskandidat vorhanden

journal.setup.identity.program.folder = Eine Neuinstallation oder Deinstallation entfernt diesen Programmordner. Wähle einen Ort für das Journal außerhalb davon. Wenn dein Journal schon dort liegt, beende die Journal-App. Wenn die Ersteinrichtung abgeschlossen ist, führe `solstone journal service stop` aus, dann `solstone journal service uninstall`, und warte, bis es gelingt. Verschiebe den ganzen Journal-Ordner vor dem Neuinstallieren oder Deinstallieren aus dem Programmordner und wähle dann den neuen Ort mit `solstone journal setup --accept-existing-journal --journal "D:\journal"`, wobei du D:\journal durch den neuen Ort ersetzt.

journal.setup.identity.recover = diese Installation konnte nicht überprüft werden.{ $paragraph }Details: { $error }{ $paragraph }zum Wiederherstellen halte den Dienst an, entferne die Artefakte der Ersteinrichtung dieser Installation und führe `{ $setup }` erneut aus:{ $stop_service }{ $steps }{ $paragraph }dein Journal selbst ist unberührt. keines davon hält deine Erinnerungen.

journal.setup.identity.spki.invalid = ungültige Journal-SPKI: { $detail }

journal.setup.identity.state.ca.bundle.is.not.a.directory = ungültiger Zustand des lokalen Links: CA-Bundle ist kein Verzeichnis

journal.setup.identity.state.candidate.ca.is.not.a.directory = ungültiger lokaler Verknüpfungszustand: Kandidaten-CA ist kein Verzeichnis

journal.setup.identity.state.candidate.cert.pem = ungültiger Zustand des lokalen Links: { $candidate_cert }: { $error }

journal.setup.identity.state.candidate.directory = ungültiger Zustand des lokalen Links: Kandidatenverzeichnis: { $error }

journal.setup.identity.state.candidate.private.pem = ungültiger lokaler Linkstatus: { $candidate_key }: { $error }

journal.setup.identity.state.cert.pem = ungültiger lokaler Linkstatus: { $cert_file }: { $error }

journal.setup.identity.state.identity.directory = ungültiger lokaler Verknüpfungszustand: Identitätsverzeichnis: { $error }

journal.setup.identity.state.private.pem = ungültiger lokaler Linkstatus: { $key_file }: { $error }

journal.setup.identity.state.state.instance.id.does.not.match.the.ca.public.key = ungültiger lokaler Link-Zustand: Zustand { $identity_key } passt nicht zum öffentlichen CA-Schlüssel

journal.setup.identity.state.state.json = ungültiger Zustand des lokalen Links: { $state_file }: { $error }

journal.setup.identity.state.state.json.instance.id.is.missing = ungültiger Zustand des lokalen Links: { $state_file } { $identity_key } fehlt

journal.setup.identity.state.state.json.locked.at.is.missing = ungültiger lokaler Link-Zustand: { $state_file } { $locked_key } fehlt

journal.setup.identity.state.state.json.must.be.an.object = ungültiger lokaler Link-Zustand: { $state_file } muss ein Objekt sein

journal.setup.identity.uncertain = diese Installation konnte nicht überprüft werden.{ $paragraph }Details: { $error }{ $paragraph }die Ersteinrichtung konnte nicht erkennen, welche Installation zu den Kommandozeilenwerkzeugen oder zum Hintergrund-Support gehört. lass sie an ihrem Ort und nimm diese Details in eine Support-Anfrage auf.

journal.setup.identity.windows = diese Installation ließ sich nicht überprüfen.{ $paragraph }Details: { $error }{ $paragraph }zur Wiederherstellung zuerst `{ $uninstall }` ausführen. bei Fehler hier anhalten und die Details der Support-Anfrage beilegen. bei Erfolg Befehle in { $shell } ausführen, dann `{ $setup }` erneut:{ $commands }{ $paragraph }dein Journal selbst ist unberührt. keines davon hält deine Erinnerungen.

journal.setup.installer.cleanup.failed = Bereinigung der Zulassung des Installationsprogramms ist nicht gelungen: { $error }

journal.setup.installer.control.timed.out = Zeitüberschreitung bei der Steuerung des Installationsprogramms

journal.setup.installer.control.unavailable = Steuerung des Installationsprogramms nicht verfügbar

journal.setup.installer.directory.unavailable = Installationsverzeichnis nicht verfügbar

journal.setup.installer.exited = das Installationsprogramm wurde vor der Zulassung beendet ({ $code })

journal.setup.installer.identity.unavailable = Identität des Installationsprogramms nicht verfügbar

journal.setup.installer.job.authority = Job-Autorität des Installationsprogramms nicht verfügbar

journal.setup.installer.job.cleanup = Bereinigung des Jobs des Installationsprogramms ist nicht gelungen: { $error }

journal.setup.installer.lease.busy = Lease des Installationsprogramms ist belegt

journal.setup.installer.process.identity.invalid = Prozessidentität des Installationsprogramms ungültig

journal.setup.installer.process.identity.unavailable = Prozessidentität des Installationsprogramms nicht verfügbar

journal.setup.installer.process.observation.unavailable = Prozessbeobachtung des Installationsprogramms nicht verfügbar

journal.setup.installer.registry.unavailable = Register des Installationsprogramms nicht verfügbar

journal.setup.installer.still.running = Installationsprogramm läuft noch

journal.setup.installer.stopped = Installationsprogramm wurde gestoppt

journal.setup.installer.timed.out = Zeitüberschreitung bei der Zulassung des Installationsprogramms

journal.setup.installer.tree.not.reaped = Prozessbaum nicht abgeräumt: { $reason }; verbleibend={ $survivors }

journal.setup.journal.aborted = Ersteinrichtung von dir abgebrochen

journal.setup.journal.existing = { $setup }: kann im nicht-interaktiven Modus nicht fortfahren - { $path } enthält bereits Journaldaten.{ $line }Die Ersteinrichtung beansprucht ein vorhandenes Journal nicht automatisch.{ $paragraph }Versuch es erneut mit einem von:{ $indent }{ $accept }{ $indent }{ $new }{ $paragraph }Interaktiver Ausweg:{ $indent }{ $verb }{ $paragraph }Führe { $explain } für die ganze Schrittliste aus.

journal.setup.journal.existing.installer = { $setup }: kann im nicht-interaktiven Modus nicht fortfahren - { $path } enthält schon Journal-Daten.{ $line }Die Ersteinrichtung übernimmt ein vorhandenes Journal nicht automatisch.{ $paragraph }Erneut versuchen mit einem von:{ $indent }{ $accept }{ $indent }{ $new }{ $paragraph }Interaktiver Ausweg:{ $indent }{ $verb }{ $paragraph }Nach jedem davon denselben { $installer }-Befehl erneut ausführen, um die Installation abzuschließen.{ $paragraph }Führe { $explain } für die vollständige Schrittliste aus.

journal.setup.journal.path.file = unter { $path } ein Verzeichnis erwartet, eine reguläre Datei erhalten. Erneut mit { $option }.

journal.setup.login.help = automatisch starten, nachdem du dich am Computer deines Journals angemeldet hast.

journal.setup.login.label = dein Journal starten, wenn du dich anmeldest

journal.setup.mark.error = mark deines Journals ließ sich nicht laden. versuch es noch einmal.

journal.setup.mark.help = eindeutige mark deines Journals. Telefon oder anderer Computer, den du koppelst, zeigt sie auch, damit du sie prüfst.

journal.setup.mark.permanence = fortfahren macht dies endgültig zur mark deines Journals.

journal.setup.mark.title = die mark deines Journals

journal.setup.mark.try.another = eine andere versuchen

journal.setup.refusal.action.is.required = Aktion ist erforderlich

journal.setup.refusal.attempt.id.is.required = { $attempt_id_value } ist erforderlich

journal.setup.refusal.bad.request = Ungültige Anfrage

journal.setup.refusal.choice.is.required = Auswahl ist erforderlich

journal.setup.refusal.choose.local.thinking.before.starting.its.download = wähle lokales Denken, bevor du seinen Download startest

journal.setup.refusal.choose.thinking.before.finishing.setup = wähle Denken, bevor du die Ersteinrichtung abschließt

journal.setup.refusal.clear.your.own.endpoint.first.to.run.the.bundled.local.model = leere zuerst den eigenen endpoint, um das mitgelieferte lokale Modell zu starten.

journal.setup.refusal.conflict = Konflikt

journal.setup.refusal.forbidden = Verboten

journal.setup.refusal.identity.proposal.conflict = Konflikt beim Identitätsvorschlag

journal.setup.refusal.internal.server.error = Interner Serverfehler

journal.setup.refusal.invalid.timezone = ungültige Zeitzone

journal.setup.refusal.journal.id.already.locked = Journal-ID ist bereits gesperrt

journal.setup.refusal.journal.id.must.be.locked.before.setup.can.finish = Journal-ID muss vor dem Ende der Ersteinrichtung gesperrt sein

journal.setup.refusal.lane.must.be.one.of.byo.confidential.local = lane muss eines davon sein: { $byo_value }, confidential, local

journal.setup.refusal.local.thinking.is.not.available.on.your.journal.s.computer = lokales Denken ist auf dem Computer deines Journals nicht verfügbar

journal.setup.refusal.malformed.json.request = fehlerhafte JSON-Anfrage

journal.setup.refusal.malformed.lock.request = fehlerhafte Sperranforderung

journal.setup.refusal.not.found = nicht gefunden

journal.setup.refusal.owner.identity.failed = dein Profil für das Journal konnte nicht gespeichert werden

journal.setup.refusal.proposal.id.is.required = { $proposal_id_value } ist erforderlich

journal.setup.refusal.retention.days.must.be.a.positive.integer = retention_days muss eine positive ganze Zahl sein

journal.setup.refusal.retry.continues.the.same.backup.restore = ein erneuter Versuch setzt dieselbe Wiederherstellung der Sicherung fort.

journal.setup.refusal.service.unavailable = Dienst nicht verfügbar

journal.setup.refusal.settings.lock.unavailable = Einstellungssperre nicht verfügbar

journal.setup.refusal.settings.read.failed = Einstellungen ließen sich nicht lesen

journal.setup.refusal.settings.write.failed = Schreiben der Einstellungen ist nicht gelungen

journal.setup.refusal.setup.routes.require.a.localhost.connection = Ersteinrichtungsrouten brauchen eine localhost-Verbindung

journal.setup.refusal.setup.target.use.was.refused = Nutzung des Ersteinrichtungsziels abgelehnt

journal.setup.refusal.speech.recognition.needs.repair = Spracherkennung muss repariert werden

journal.setup.refusal.the.restore.didn.t.complete = die Wiederherstellung wurde nicht abgeschlossen

journal.setup.refusal.thinking.choice.is.required.before.setup.can.finish = Wahl fürs Denken nötig, bevor die Ersteinrichtung fertig werden kann

journal.setup.refusal.thinking.download.could.not.start = das Herunterladen fürs Denken konnte nicht starten

journal.setup.refusal.this.journal.is.still.being.restored.and.isn.t.ready.to.finish.setup = dieses Journal wird noch wiederhergestellt und kann die Ersteinrichtung nicht beenden.

journal.setup.refusal.this.journal.is.still.being.restored.and.isn.t.ready.to.use = dieses Journal wird noch wiederhergestellt und ist nicht bereit zur Nutzung.

journal.setup.refusal.this.journal.isn.t.ready.to.use = dieses Journal ist nicht bereit zur Nutzung.

journal.setup.refusal.unknown.action = unbekannte Aktion

journal.setup.refusal.unknown.thinking.choice = unbekannte Wahl fürs Denken

journal.setup.refusal.whole.journal.restore.is.pending = ganze Journal-Wiederherstellung steht noch aus

journal.setup.refusal.your.journal.s.identity.couldn.t.be.set.up = die Identität deines Journals ließ sich nicht einrichten.

journal.setup.refusal.your.journal.settings.could.not.be.read = deine Journal-Einstellungen konnten nicht gelesen werden

journal.setup.refusal.your.thinking.choice.changed.review.it.before.trying.again = deine Wahl beim Denken hat sich geändert; prüfe sie vor einem erneuten Versuch

journal.setup.refusal.your.thinking.settings.changed.open.thinking.to.choose.what.to.use = deine Einstellungen fürs Denken haben sich geändert; öffne Denken, wähl, was du nutzt

journal.setup.service.failed = Dienststart ist nicht gelungen (exit { $status })

journal.setup.speech.cannot.start = Installation für Gesprochenes konnte nicht starten

journal.setup.speech.check.progress = prüfe den Fortschritt beim Gesprochenen gleich.

journal.setup.speech.retry.unavailable = erneute Spracherkennung ist nicht verfügbar.

journal.setup.speech.starting = Installation für Gesprochenes startet

journal.setup.speech.unavailable = erneuter Versuch nicht verfügbar

journal.setup.step.exit = { $step }-Schritt wurde mit Code { $status } beendet

journal.setup.step.failed = Schritt fehlgeschlagen

journal.setup.step.timeout = { $step }-Schritt hat nach { $seconds } s die Zeit überschritten

journal.setup.step.warning = Schrittwarnung

journal.setup.thinking.change = ändern

journal.setup.thinking.check.unknown = lokale Ersteinrichtung ließ sich nicht prüfen.

journal.setup.thinking.choose.later = wähle das Denken, wenn du bereit bist. dein Journal behält bis dahin alles.

journal.setup.thinking.default.line = auf dem Computer deines Journals · { $size }

journal.setup.thinking.later = später entscheiden

journal.setup.thinking.size.unknown = Downloadgröße nicht verfügbar

journal.setup.welcome.body = die { $platform_name }-App nimmt entgegen, was du mit ihr teilst, und alles davon geht in dein Journal.

journal.setup.welcome.title = willkommen bei { $platform_name }.

journal.setup.wrapper.legacy = der Dienst startet weiterhin über { $path }

journal.setup.wrapper.legacy.retry = die Ersteinrichtung entfernt diesen Wrapper, nachdem der Dienst durch { $path } startet.

journal.setup.wrapper.provision = { $command }-Wrapper ließ sich unter { $path } nicht bereitstellen ({ $kind }: { $error })

journal.setup.wrapper.provision.remedy = korrigiere die Berechtigungen für ~/.local/bin und führe `solstone journal setup` erneut aus, oder rufe solstone direkt aus der Laufzeitumgebung auf

journal.setup.wrapper.retire.failed = die Befehlszeilenwerkzeuge der älteren Version ließen sich nicht entfernen: { $error }

journal.setup.wrapper.retry = die Ersteinrichtung versucht es beim nächsten Mal erneut.

journal.window.body.accessible.name.body.views = Körperansichten

journal.window.body.accessible.name.close = schließen

journal.window.body.accessible.name.days.your.journal.holds.body.data = Tage, an denen dein Journal Körperdaten hält

journal.window.body.accessible.name.empty.day = leerer Tag

journal.window.body.button.label.delete = löschen

journal.window.body.count.day = { $quantity ->
        [one] { $count } Tag
       *[other] { $count } Tage
    }

journal.window.body.count.entry = { $quantity ->
        [one] { $count } Eintrag
       *[other] { $count } Einträge
    }

journal.window.body.count.interval = { $quantity ->
    [one] { $count } Intervall
   *[other] { $count } Intervalle
}

journal.window.body.count.reading = { $quantity ->
    [one] { $count } Messwert
   *[other] { $count } Messwerte
}

journal.window.body.date.clock.afternoon = { $hour }:{ $minute } nachm.

journal.window.body.date.clock.morning = { $hour }:{ $minute } vorm.

journal.window.body.date.clock.range = { $first_clock }–{ $last_clock }

journal.window.body.date.full = {$day}.{ $slot1 }{$month}{ $slot2 }{$year}

journal.window.body.date.month.year = {$month}{ $slot1 }{$year}

journal.window.body.date.short = {$month}{ $slot1 }{$day}

journal.window.body.date.time.before = {$date},{ $slot1 }{$time}

journal.window.body.date.with.clock = {$day}.{ $slot1 }{$month}{ $slot2 }{$year},{ $slot3 }{$time}

journal.window.body.day.invalid = dieser Tag konnte nicht verwendet werden.

journal.window.body.day.invalid.detail = Ungültiger Tag

journal.window.body.day.nearby.with.data = benachbarte Tage mit Daten: { $nearby_days }

journal.window.body.document.dated = für dieses Datum: { $name } { $value }

journal.window.body.document.dated.list = { $quantity ->
    [one] Tagesdokument von { $source } für dieses Datum: { $leading } und { $last }
   *[other] Tagesdokumente von { $source } für dieses Datum: { $leading } und { $last }
}

journal.window.body.document.dated.one = { $quantity ->
    [one] Tagesdokument von { $source } für dieses Datum: { $last }
   *[other] Tagesdokumente von { $source } für dieses Datum: { $last }
}

journal.window.body.document.experience.continuing.generic = Erlebnis läuft weiter: { $name }, { $start } bis { $end }

journal.window.body.document.experience.continuing.generic.recorder = Erlebnis läuft weiter: { $name }, { $start } bis { $end } ({ $recorder })

journal.window.body.document.experience.continuing.named = { $kind } läuft weiter: { $name }, { $start } bis { $end }

journal.window.body.document.experience.continuing.named.recorder = { $kind } fortlaufend: { $name }, { $start } bis { $end } ({ $recorder })

journal.window.body.document.experience.generic = Erlebnis: { $name }, { $start } bis { $end }

journal.window.body.document.experience.generic.recorder = Erlebnis: { $name }, { $start } bis { $end } ({ $recorder })

journal.window.body.document.experience.generic.recorder.words = Erlebnis: { $name }, { $start } bis { $end } ({ $recorder }): { $words }

journal.window.body.document.experience.generic.words = Erlebnis: { $name }, { $start } bis { $end }: { $words }

journal.window.body.document.experience.named = { $kind }: { $name }, { $start } bis { $end }

journal.window.body.document.experience.named.recorder = { $kind }: { $name }, { $start } bis { $end } ({ $recorder })

journal.window.body.document.experience.named.recorder.words = { $kind }: { $name }, { $start } bis { $end } ({ $recorder }): { $words }

journal.window.body.document.experience.named.words = { $kind }: { $name }, { $start } bis { $end }: { $words }

journal.window.body.document.heading = Körper am { $weekday }, { $day }. { $month } { $year }

journal.window.body.document.heading.raw = Körper am { $day }

journal.window.body.document.tally.count.plain.at = { $quantity ->
    [one] { $name }: { $count } Intervall, { $start }
   *[other] { $name }: Intervalle { $count }, { $start }
}

journal.window.body.document.tally.count.plain.count = { $quantity ->
    [one] { $name }: { $count } Intervall
   *[other] { $name }: Intervalle { $count }
}

journal.window.body.document.tally.count.plain.range = { $quantity ->
    [one] { $name }: { $count } Intervall, { $start } bis { $end }
   *[other] { $name }: { $count } Intervalle, { $start } bis { $end }
}

journal.window.body.document.tally.count.recorder.at = { $quantity ->
    [one] { $name }: { $count } Intervall, { $start } ({ $recorder })
   *[other] { $name }: Intervalle { $count }, { $start } ({ $recorder })
}

journal.window.body.document.tally.count.recorder.count = { $quantity ->
    [one] { $name }: { $count } Intervall ({ $recorder })
   *[other] { $name }: Intervalle { $count } ({ $recorder })
}

journal.window.body.document.tally.count.recorder.range = { $quantity ->
    [one] { $name }: { $count } Intervall, { $start } bis { $end } ({ $recorder })
   *[other] { $name }: Intervalle { $count }, { $start } bis { $end } ({ $recorder })
}

journal.window.body.document.tally.logged.owner.at = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start } (von dir eingetragen)
   *[other] { $name }: Einträge { $count }, { $start } (von dir eingetragen)
}

journal.window.body.document.tally.logged.owner.count = { $quantity ->
    [one] { $name }: { $count } Eintrag (von dir eingetragen)
   *[other] { $name }: Einträge { $count } (von dir eingetragen)
}

journal.window.body.document.tally.logged.owner.range = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start } bis { $end } (von dir eingetragen)
   *[other] { $name }: Einträge { $count }, { $start } bis { $end } (von dir eingetragen)
}

journal.window.body.document.tally.logged.owner.recorder.at = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start } (von dir in { $recorder } eingetragen)
   *[other] { $name }: Einträge { $count }, { $start } (von dir in { $recorder } eingetragen)
}

journal.window.body.document.tally.logged.owner.recorder.count = { $quantity ->
    [one] { $name }: { $count } Eintrag (von dir eingetragen in { $recorder })
   *[other] { $name }: Einträge { $count } (von dir eingetragen in { $recorder })
}

journal.window.body.document.tally.logged.owner.recorder.range = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start } bis { $end } (von dir in { $recorder } eingetragen)
   *[other] { $name }: Einträge { $count }, { $start } bis { $end } (von dir in { $recorder } eingetragen)
}

journal.window.body.document.tally.logged.plain.at = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start }
   *[other] { $name }: Einträge { $count }, { $start }
}

journal.window.body.document.tally.logged.plain.count = { $quantity ->
    [one] { $name }: { $count } Eintrag
   *[other] { $name }: { $count } Einträge
}

journal.window.body.document.tally.logged.plain.range = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start } bis { $end }
   *[other] { $name }: Einträge { $count }, { $start } bis { $end }
}

journal.window.body.document.tally.logged.recorder.at = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start } (aufgezeichnet von { $recorder })
   *[other] { $name }: Einträge { $count }, { $start } (aufgezeichnet von { $recorder })
}

journal.window.body.document.tally.logged.recorder.count = { $quantity ->
    [one] { $name }: { $count } Eintrag (aufgezeichnet von { $recorder })
   *[other] { $name }: { $count } Einträge (aufgezeichnet von { $recorder })
}

journal.window.body.document.tally.logged.recorder.range = { $quantity ->
    [one] { $name }: { $count } Eintrag, { $start } bis { $end } (aufgezeichnet von { $recorder })
   *[other] { $name }: Einträge { $count }, { $start } bis { $end } (aufgezeichnet von { $recorder })
}

journal.window.body.document.tally.reading.plain.at = { $quantity ->
    [one] { $name }: { $count } Messwert, { $start }
   *[other] { $name }: Messwerte { $count }, { $start }
}

journal.window.body.document.tally.reading.plain.count = { $quantity ->
    [one] { $name }: { $count } Messwert
   *[other] { $name }: Messwerte { $count }
}

journal.window.body.document.tally.reading.plain.range = { $quantity ->
    [one] { $name }: { $count } Messwert, { $start } bis { $end }
   *[other] { $name }: { $count } Messwerte, { $start } bis { $end }
}

journal.window.body.document.tally.reading.recorder.at = { $quantity ->
    [one] { $name }: { $count } Messwert, { $start } ({ $recorder })
   *[other] { $name }: Messwerte { $count }, { $start } ({ $recorder })
}

journal.window.body.document.tally.reading.recorder.count = { $quantity ->
    [one] { $name }: { $count } Messwert ({ $recorder })
   *[other] { $name }: { $count } Messwerte ({ $recorder })
}

journal.window.body.document.tally.reading.recorder.range = { $quantity ->
    [one] { $name }: { $count } Messwert, { $start } bis { $end } ({ $recorder })
   *[other] { $name }: Messwerte { $count }, { $start } bis { $end } ({ $recorder })
}

journal.window.body.document.tally.stage.plain.at = { $quantity ->
    [one] Schlafphase { $name }: { $count } Intervall, { $start }
   *[other] Schlafphase { $name }: Intervalle { $count }, { $start }
}

journal.window.body.document.tally.stage.plain.count = { $quantity ->
    [one] Schlafphase { $name }: { $count } Intervall
   *[other] Schlafphase { $name }: Intervalle { $count }
}

journal.window.body.document.tally.stage.plain.range = { $quantity ->
    [one] Schlafphase { $name }: { $count } Intervall, { $start } bis { $end }
   *[other] Schlafphase { $name }: Intervalle { $count }, { $start } bis { $end }
}

journal.window.body.document.tally.stage.recorder.at = { $quantity ->
    [one] Schlafphase { $name }: { $count } Intervall, { $start } ({ $recorder })
   *[other] Schlafphase { $name }: { $count } Intervalle, { $start } ({ $recorder })
}

journal.window.body.document.tally.stage.recorder.count = { $quantity ->
    [one] Schlafphase { $name }: { $count } Intervall ({ $recorder })
   *[other] Schlafphase { $name }: { $count } Intervalle ({ $recorder })
}

journal.window.body.document.tally.stage.recorder.range = { $quantity ->
    [one] Schlafphase { $name }: { $count } Intervall, { $start } bis { $end } ({ $recorder })
   *[other] Schlafphase { $name }: Intervalle { $count }, { $start } bis { $end } ({ $recorder })
}

journal.window.body.document.words.owner = in deinen Worten: { $words }

journal.window.body.document.words.source = in den Worten der Quelle: { $words }

journal.window.body.empty.workouts.import = hier ist noch nichts. hol deine Trainings aus einem Strava-Download auf der { $import_link_start }Seite Import{ $import_link_end } herein.

journal.window.body.empty.workouts.import.resources = wenn dein Journal auf einem Computer läuft, kannst du auch einen Export von Apple Health auf der Seite Import oder Daten von Oura über die Befehlszeile dieses Computers hereinholen. { $docs_link_start }über Körper-Importe lesen{ $docs_link_end }.

journal.window.body.error.day.detail = Ungültiger Tag

journal.window.body.error.day.invalid = dieser Tag konnte nicht verwendet werden.

journal.window.body.error.hidden.body = Body muss ein JSON-Objekt sein

journal.window.body.error.hidden.days.empty = Tage müssen mindestens einen Tag nennen

journal.window.body.error.hidden.days.list = Tage müssen eine Liste von Tagen sein

journal.window.body.error.hidden.hidden = hidden muss true oder false sein

journal.window.body.error.hidden.unreadable = dein Journal kann nicht lesen, welche Tage du ausgeblendet hast.

journal.window.body.error.hidden.unreadable.detail = die ausgeblendeten Tage des Körpers können nicht gelesen werden

journal.window.body.error.hidden.unsaved = diese Änderung konnte nicht gespeichert werden.

journal.window.body.error.invalid.request = einer dieser Werte konnte nicht verwendet werden.

journal.window.body.error.limit.integer = das Limit muss eine ganze Zahl sein

journal.window.body.error.limit.minimum = Limit muss mindestens 1 sein

journal.window.body.error.month.invalid = Ungültiger Monat

journal.window.body.error.unreadable = die Körper-App konnte dein Journal nicht lesen.

journal.window.body.error.window.bounds = Zeitfenster braucht gültige ISO-Zeitstempel für Anfang und Ende.

journal.window.body.error.window.order = Fensterende muss nach dem Fensteranfang liegen.

journal.window.body.error.window.span = { $quantity ->
    [one] Zeitfenster muss { $maximum_days } Tag oder weniger sein.
   *[other] Zeitfenster muss { $maximum_days } Tage oder weniger sein.
}

journal.window.body.experience.continuing.generic = Erlebnis fortlaufend

journal.window.body.experience.continuing.named = { $experience_kind } geht weiter

journal.window.body.experience.fallback = Erlebnis

journal.window.body.grid.empty = {$date}{ $slot1 }· hier ist nichts

journal.window.body.grid.held = {$date}{ $slot1 }· zurückgehalten in einem früheren Import

journal.window.body.grid.held.month = {$date}{ $slot1 }· in einem Monat, den ein früherer Import hält

journal.window.body.grid.hidden = {$date}{ $slot1 }· du hast diesen Tag ausgeblendet

journal.window.body.grid.source.all = { $date } · { $slot1 }, { $slot2 } und { $slot3 }

journal.window.body.grid.source.apple = { $date } · { $slot1 }

journal.window.body.grid.source.apple.oura = { $date } · { $slot1 } und { $slot2 }

journal.window.body.grid.source.apple.strava = { $date } · { $slot1 } und { $slot2 }

journal.window.body.grid.source.oura = { $date } · { $slot1 }

journal.window.body.grid.source.oura.strava = { $date } · { $slot1 } und { $slot2 }

journal.window.body.grid.source.strava = { $date } · { $slot1 }

journal.window.body.hide.copy.grid.foot.striped.days.are.ones.you.hid = gestreifte Tage sind die, die du ausgeblendet hast.

journal.window.body.hide.copy.hide.failed.couldn.t.hide.this.day.nothing = dieser Tag konnte nicht ausgeblendet werden. nichts hat sich geändert.

journal.window.body.hide.copy.hide.hide.this.day = diesen Tag ausblenden

journal.window.body.hide.copy.mark.head.you.hid.this.day.s.body = du hast die Körperdaten dieses Tages ausgeblendet

journal.window.body.hide.copy.mark.line1.it.s.still.in.your.journal = es ist noch in deinem Journal, und es bleibt ausgeblendet, wenn du es erneut importierst oder abgleichst.

journal.window.body.hide.copy.mark.line2.search.and.the.activities.page.leave = die Suche und die Seite Aktivitäten lassen ihn aus, und verbundene Agenten können ihn nicht erreichen. eine Nacht, die an diesem Tag beginnt, ist mit ihm ausgeblendet, auch nach Mitternacht.

journal.window.body.hide.copy.mark.unhide.unhide.this.day = diesen Tag wieder einblenden

journal.window.body.hide.copy.no.answer.hide.it.may.already.be.hidden.reload = es ist vielleicht schon ausgeblendet. lade neu zum Prüfen.

journal.window.body.hide.copy.no.answer.unhide.it.may.already.be.unhidden.reload = es ist vielleicht schon wieder eingeblendet. lade neu, um zu prüfen.

journal.window.body.hide.copy.no.answer.your.journal.didn.t.answer = dein Journal hat nicht geantwortet

journal.window.body.hide.copy.receipt.message.it.stays.hidden.when.you.import = es bleibt ausgeblendet, wenn du es erneut importierst oder abgleichst.

journal.window.body.hide.copy.receipt.no.search.search.and.connected.agents.may.still = die Suche und verbundene Agenten können es noch finden.

journal.window.body.hide.copy.try.again.try.again = erneut versuchen

journal.window.body.hide.copy.unhide.failed.couldn.t.unhide.this.day.nothing = dieser Tag ließ sich nicht wieder einblenden. nichts hat sich geändert.

journal.window.body.hide.copy.unhide.unhide = wieder einblenden

journal.window.body.hide.copy.unreadable.your.journal.can.t.read.which = dein Journal kann nicht lesen, welche Tage du ausgeblendet hast, so erscheint hier jeder Tag, und du kannst keinen ausblenden oder wieder einblenden. bis es kann, lassen Suche und verbundene Agenten alle deine Körperdaten aus.

journal.window.body.hide.data.from.day = Körperdaten von { $day } ausblenden

journal.window.body.hide.data.receipt = Körperdaten von { $day } sind ausgeblendet.

journal.window.body.hide.previous.day.carryover = der Rest einer Nacht oder eines Trainings von { $hidden_day_link }, einem Tag, den du ausgeblendet hast, ist hier auch ausgeblendet.

journal.window.body.import.again.apple.health = erneut aus einem neuen Export von Apple Health importieren, um es hereinzuholen.

journal.window.body.import.again.dexcom = es gibt jetzt kein Importprogramm für Dexcom Clarity, also bleibt es, wie es ist.

journal.window.body.import.again.oura = importiere erneut aus deinem Oura-Konto, um es hereinzubringen.

journal.window.body.import.old.day = früherer{ $slot1 }{$source}{ $slot2 }Import hält Körperdaten dieses Tages, behalten, hier ungelesen.{ $slot3 }{$action}

journal.window.body.import.old.listing = {$source}{ $slot1 }von{ $slot2 }{$span}, behalten, aber hier nicht gelesen.{ $slot3 }{$action}

journal.window.body.import.old.month = ein früherer{ $slot1 }{$source}{ $slot2 }Import hält Körperdaten für{ $slot3 }{$month}, behalten, aber hier nicht gelesen, deshalb erscheint es auf dieser Seite nicht.{ $slot4 }{$action}

journal.window.body.import.progress.running.days = { $total_quantity ->
    [one] { $done_quantity ->
        [one] läuft noch: { $done_days } von { $total_days } Tag erledigt.
       *[other] läuft noch: { $done_days } von { $total_days } Tag erledigt.
    }
   *[other] { $done_quantity ->
        [one] läuft noch: { $done_days } von { $total_days } Tagen erledigt.
       *[other] läuft noch: { $done_days } von { $total_days } Tagen erledigt.
    }
}

journal.window.body.import.progress.running.empty = läuft noch, und noch ist nichts gespeichert.

journal.window.body.import.progress.running.entries = { $quantity ->
    [one] läuft noch: { $entry_count } Eintrag bisher gelesen.
   *[other] läuft noch: { $entry_count } Einträge bisher gelesen.
}

journal.window.body.import.progress.stopped.days = { $total_quantity ->
    [one] { $done_quantity ->
        [one] auf halbem Weg gestoppt: { $done_days } von { $total_days } Tag erledigt.
       *[other] auf halbem Weg gestoppt: { $done_days } von { $total_days } Tag erledigt.
    }
   *[other] { $done_quantity ->
        [one] auf halbem Weg gestoppt: { $done_days } von { $total_days } Tagen erledigt.
       *[other] auf halbem Weg gestoppt: { $done_days } von { $total_days } Tagen erledigt.
    }
}

journal.window.body.import.progress.stopped.empty = auf halbem Weg gestoppt, und noch ist nichts gespeichert.

journal.window.body.import.progress.stopped.entries = { $quantity ->
    [one] auf halbem Weg gestoppt: { $entry_count } Eintrag bisher gelesen.
   *[other] auf halbem Weg gestoppt: { $entry_count } Einträge bisher gelesen.
}

journal.window.body.import.span.counted.days = { $quantity ->
    [one] { $first } bis { $last }, { $count } Tag
   *[other] { $first } bis { $last }, { $count } Tage
}

journal.window.body.import.span.counted.months = { $quantity ->
    [one] { $first } bis { $last } ({ $count } Monat)
   *[other] { $first } bis { $last } ({ $count } Monate)
}

journal.window.body.import.span.month = {$month}{ $slot1 }(monatsweise)

journal.window.body.import.span.unknown = eine unbekannte Spanne

journal.window.body.link.label.body.overview = Körper-Überblick →

journal.window.body.link.label.body.overview.variant2 = ← Körperüberblick

journal.window.body.link.label.body.overview.variant3 = Körperüberblick

journal.window.body.logged.who.by = du, in{ $slot1 }{$source}

journal.window.body.logged.who.you = du

journal.window.body.message.it.may.already.be.back.in = es ist vielleicht schon wieder in deinem Journal. neu laden, um zu prüfen.

journal.window.body.message.it.may.already.be.erased.reload = es ist vielleicht schon endgültig gelöscht. lade neu, um zu prüfen.

journal.window.body.message.it.may.already.be.in.the = es kann schon im Papierkorb liegen. lade neu, um zu prüfen.

journal.window.body.message.it.s.back.in.your.journal = es ist wieder in deinem Journal.

journal.window.body.message.it.s.in.the.trash.you = es ist im Papierkorb. du kannst es von dort noch einmal versuchen.

journal.window.body.message.it.s.still.in.the.trash = es ist immer noch im Papierkorb. du kannst es von dort noch einmal versuchen.

journal.window.body.message.it.s.still.in.your.journal = es ist immer noch in deinem Journal.

journal.window.body.section.heading.counts = Anzahlen

journal.window.body.section.heading.earlier.imports.kept.but.not.read = frühere Importe, behalten, aber hier nicht gelesen

journal.window.body.section.heading.every.day.held = jeder gehaltene Tag

journal.window.body.section.heading.for.this.date = für dieses Datum

journal.window.body.section.heading.imports = Importe

journal.window.body.section.heading.logged = eingetragen

journal.window.body.section.heading.readings = Messwerte

journal.window.body.section.heading.sources.held = zurückgehaltene Quellen

journal.window.body.section.heading.strava = Strava

journal.window.body.sleep.stage = Schlafphase{ $slot1 }{$stage}

journal.window.body.status.text.couldn.t.load.body.day = Tag im Körper konnte nicht geladen werden

journal.window.body.status.text.couldn.t.load.your.body.data = deine Körperdaten konnten nicht geladen werden

journal.window.body.strava.commute = Arbeitsweg

journal.window.body.strava.distance = {$distance}{ $slot1 }km

journal.window.body.strava.duration.hours = {$hours}{ $slot1 }h

journal.window.body.strava.duration.minutes = {$minutes}{ $slot1 }min

journal.window.body.strava.duration.time = {$hours}{ $slot1 }h{ $slot2 }{$minutes}{ $slot3 }min

journal.window.body.strava.elevation = Höhengewinn{ $slot1 }{$metres}{ $slot2 }m

journal.window.body.strava.energy = {$value}{ $slot1 }kcal

journal.window.body.strava.heart = Puls Ø{ $slot1 }{$average}, max{ $slot2 }{$maximum}{ $slot3 }bpm

journal.window.body.strava.heart.average = Puls Ø{ $slot1 }{$value}{ $slot2 }bpm

journal.window.body.strava.heart.maximum = max. Puls{ $slot1 }{$value}{ $slot2 }bpm

journal.window.body.strava.manual = von Hand eingetragen

journal.window.body.strava.moving.hours = in Bewegung { $hours } h

journal.window.body.strava.moving.minutes = in Bewegung { $minutes } min

journal.window.body.strava.moving.time = in Bewegung { $hours } h { $minutes } min

journal.window.body.strava.power = Leistung Ø{ $slot1 }{$average}{ $slot2 }W, gewichtet{ $slot3 }{$weighted}{ $slot4 }W

journal.window.body.strava.power.average = mittlere Leistung{ $slot1 }{$value}{ $slot2 }W

journal.window.body.strava.power.weighted = gewichtete Leistung{ $slot1 }{$value}{ $slot2 }W

journal.window.body.strava.workout = Training

journal.window.body.title.couldn.t.erase.that.workout = dieses Training konnte nicht endgültig gelöscht werden

journal.window.body.title.couldn.t.move.that.to.the = das konnte nicht in den Papierkorb verschoben werden

journal.window.body.title.couldn.t.recover.that.workout = dieses Training konnte nicht wiederhergestellt werden

journal.window.body.title.delete.this.workout = dieses Training löschen?

journal.window.body.title.erase.this.workout.now = dieses Training jetzt endgültig löschen?

journal.window.body.title.moved.to.the.trash = in den Papierkorb verschoben

journal.window.body.title.workout.erased.from.your.journal = Training endgültig aus deinem Journal gelöscht

journal.window.body.title.workout.recovered = Training wiederhergestellt

journal.window.body.title.your.journal.didn.t.answer = dein Journal hat nicht geantwortet

journal.window.body.visible.text.nothing.for.this.day = nichts für diesen Tag.

journal.window.body.visible.text.part.of.this.day.couldn.t = ein Teil dieses Tages konnte nicht gelesen werden.

journal.window.body.visible.text.strava = · Strava

journal.window.body.visible.text.strava.variant2 = Strava

journal.window.body.window.explanation.your.body.brought.into.your.journal = dein Körper, in dein Journal gebracht.

journal.window.body.window.state.text.loading.body.data = Körperdaten werden geladen…

journal.window.body.window.state.text.loading.body.day = Tag des Körpers wird geladen…

journal.window.body.workout.copy.again.importing.a.strava.download.again.brings = erneuter Import eines Strava-Downloads bringt dieses Training zurück. ein Tag, den du ausblendest, bleibt beim erneuten Import ausgeblendet.

journal.window.body.workout.copy.back.back = zurück

journal.window.body.workout.copy.cancel.cancel = abbrechen

journal.window.body.workout.copy.erase.now.erase.now = jetzt endgültig löschen

journal.window.body.workout.copy.move.to.trash.move.to.trash = in den Papierkorb verschieben

journal.window.body.workout.copy.open.trash.open.trash = Papierkorb öffnen

journal.window.body.workout.copy.reach.it.leaves.your.journal.now.and = es verlässt dein Journal jetzt und kann nicht wiederhergestellt werden.

journal.window.body.workout.copy.reach.it.leaves.your.journal.now.and.variant2 = es verlässt dein Journal jetzt, wartet im Papierkorb, dann wird es endgültig gelöscht. bis dahin kannst du es wiederherstellen.

journal.window.body.workout.copy.recover.recover = wiederherstellen

journal.window.body.workout.copy.stays.activities.summaries.and.entities.made.from = Aktivitäten, Zusammenfassungen und Entitäten daraus, anderswo in deinem Journal, werden nicht endgültig gelöscht und können es noch erwähnen.

journal.window.body.workout.copy.stays.activities.summaries.and.entities.made.from.variant2 = Aktivitäten, Zusammenfassungen und Entitäten, die anderswo in deinem Journal daraus erstellt wurden, bleiben, wo sie sind, und können es noch erwähnen.

journal.window.body.workout.delete.name = löschen { $workout_name }

journal.window.body.workout.strava.additional.count = { $quantity ->
        [one] mehr von Strava · { $count }
       *[other] mehr von Strava · { $count }
    }

journal.window.body.workout.strava.figures = Zahlen von Strava: { $figures }

journal.window.body.workout.trash.recovers.until.day = wird { $erase_day } endgültig gelöscht, außer du stellst es wieder her.

journal.window.calendar.note.added = Kalendertermin hinzugefügt

journal.window.calendar.note.added.titled = Kalenderereignis hinzugefügt: { $title }

journal.window.calendar.note.attendee.owner = { $name }, du

journal.window.calendar.note.attendee.pair = { $attendees }; { $attendee }

journal.window.calendar.note.attendee.status = { $name } ({ $status })

journal.window.calendar.note.calendar = Kalender: { $calendar }

journal.window.calendar.note.calendar.account = Kalender: { $calendar } ({ $account })

journal.window.calendar.note.changed = Kalendertermin geändert

journal.window.calendar.note.changed.titled = Kalenderereignis geändert: { $title }

journal.window.calendar.note.date = { $weekday }, { $day }. { $month } { $year }

journal.window.calendar.note.invited = Eingeladen: { $attendees }

journal.window.calendar.note.invited.more = { $remaining ->
    [one] Eingeladen: { $attendees }; und { $remaining } weitere
   *[other] Eingeladen: { $attendees }; und { $remaining } weitere
}

journal.window.calendar.note.notes = Notizen: { $notes }

journal.window.calendar.note.recorded = Aufgezeichnet: { $date }, { $time } ({ $offset })

journal.window.calendar.note.removed = Kalenderereignis entfernt

journal.window.calendar.note.removed.titled = Kalenderereignis entfernt: { $title }

journal.window.calendar.note.repeats = Wiederholung: { $repeats }

journal.window.calendar.note.when.across.days = Wann: { $start_date }, { $start_time } bis { $end_date }, { $end_time }

journal.window.calendar.note.when.across.days.zoned = Wann: { $start_date }, { $start_time } bis { $end_date }, { $end_time } ({ $zone })

journal.window.calendar.note.when.all.day = Wann: { $date }, ganztägig

journal.window.calendar.note.when.all.days = Wann: { $start_date } bis { $end_date }, ganztägig

journal.window.calendar.note.when.same.day = Wann: { $date }, { $start_time } bis { $end_time }

journal.window.calendar.note.when.same.day.zoned = Wann: { $date }, { $start_time } bis { $end_time } ({ $zone })

journal.window.calendar.note.when.time = Wann: { $date }, { $time }

journal.window.calendar.note.when.time.zoned = Wann: { $date }, { $time } ({ $zone })

journal.window.calendar.note.where = Wo: { $location }

journal.window.chrome.connection.connecting = wird verbunden…

journal.window.chrome.connection.delayed = manche Funktionen können sich verzögern

journal.window.chrome.connection.reconnecting = Verbindung verloren. wird neu verbunden

journal.window.chrome.console.action.clear = leeren

journal.window.chrome.console.action.close = schließen

journal.window.chrome.console.action.send = senden

journal.window.chrome.console.action.send.all = an den Support senden

journal.window.chrome.console.action.show.details = Details anzeigen

journal.window.chrome.console.detail.location.label = Ort

journal.window.chrome.console.detail.message.label = Nachricht

journal.window.chrome.console.detail.stack.label = Aufrufstapel

journal.window.chrome.console.empty = keine Systemnachrichten in dieser Sitzung.

journal.window.chrome.console.heading = Systemnachrichten

journal.window.chrome.console.link.label = Systemnachrichten

journal.window.chrome.console.reporting.off = das Senden von Meldungen ist aus.

journal.window.chrome.console.summary.load.failed = ein Teil dieser Seite konnte nicht geladen werden.

journal.window.chrome.console.summary.request.failed = diese Anfrage konnte nicht abgeschlossen werden.

journal.window.chrome.console.summary.unexpected = etwas auf dieser Seite hat nicht funktioniert.

journal.window.chrome.console.tab.all = alle

journal.window.chrome.console.tab.errors = Fehler

journal.window.chrome.console.tab.info = Info

journal.window.chrome.console.tab.warnings = Warnungen

journal.window.chrome.day.zones.all = die Zeiten dieses Tags sind in { $zones }

journal.window.chrome.day.zones.part = einige Uhrzeiten dieses Tages liegen in { $zones }

journal.window.chrome.log.read.failed = dieses Protokoll konnte nicht gelesen werden. lade neu und versuch es noch einmal.

journal.window.chrome.reload.hint = neu laden, um es erneut zu versuchen.

journal.window.chrome.report.action.cancel = abbrechen

journal.window.chrome.report.action.send = fortfahren zum Support

journal.window.chrome.report.app.label = App

journal.window.chrome.report.button.label = dies melden

journal.window.chrome.report.default.subject = Fehler in convey

journal.window.chrome.report.error.code.label = Fehlercode

journal.window.chrome.report.privacy.note = nichts verlässt dieses Journal, bis du das Formular auf der Support-Website sendest.

journal.window.chrome.report.recent.label = letzte Fehlerzeilen

journal.window.chrome.report.route.label = Route

journal.window.chrome.report.title = diese Meldung prüfen

journal.window.chrome.retry = erneut versuchen

journal.window.chrome.section.error = dieser Abschnitt konnte nicht geladen werden

journal.window.chrome.still.working = arbeitet noch…

journal.window.chrome.unknown.error = da ist etwas schiefgelaufen. lade neu und versuch es noch einmal.

journal.window.chrome.view.document.title = { $view ->
        [agents] Agenten - journal
        [activities] Aktivitäten - journal
        [backup] Sicherung - journal
        [body] Körper - journal
        [curation] Vorschläge - journal
        [entities] Entitäten - journal
        [health] Zustand - journal
        [home] Start - journal
        [import] Import - journal
        [map] Karte - journal
        [more] mehr - journal
        [network] Netzwerk - journal
        [search] Suche - journal
        [settings] Einstellungen - journal
        [sources] Quellen - journal
        [speakers] Sprecher - journal
        [stats] Statistiken - journal
        [support] Support - journal
        [thinking] Denken - journal
        [transcripts] Transkripte - journal
        [year] Jahr - journal
       *[other] { $fallback_label } - journal
    }

journal.window.chrome.zone.time = { $label }-Zeit

journal.window.content.action.heading = ### { $action } von { $actor }

journal.window.content.action.parameters = **Parameter:**

journal.window.content.action.source = **Quelle:** { $source }

journal.window.content.action.talent = **Talent:** [{ $id }](/app/thinking/#runs/run/{ $id })

journal.window.content.action.time = **Zeit:** { $time }

journal.window.content.action.unknown = unbekannt

journal.window.content.activity.field.activity = - Aktivität: { $value }

journal.window.content.activity.field.day = - Tag: { $value }

journal.window.content.activity.field.description = - Beschreibung: { $value }

journal.window.content.activity.field.details = - Details: { $value }

journal.window.content.activity.field.facet = - Facette: { $value }

journal.window.content.activity.field.kind = - Art: { $value }

journal.window.content.activity.field.level = - Stufe des zuletzt aktiven Segments: { $value }

journal.window.content.activity.field.time = - Zeit: { $value }

journal.window.content.activity.hidden = - Ausgeblendet: ja

journal.window.content.activity.omitted = Nicht alles, was mit dieser Aktivität gespeichert wurde, passte in diesen Eintrag. Der vollständige Datensatz der Aktivität hat alles davon.

journal.window.content.activity.role.attendee = - Teilnehmende: { $names }

journal.window.content.activity.role.heard = - Im Gespräch: { $names }

journal.window.content.activity.role.mentioned = - Erwähnt: { $names }

journal.window.content.activity.role.other = - Teilnahme: { $names }

journal.window.content.activity.role.visible = - Auf dem Bildschirm: { $names }

journal.window.content.activity.saved.relation = Gespeicherte Beziehung: { $value }

journal.window.content.activity.topics = Themen: { $topics }

journal.window.content.activity.untitled = Unbenannte Aktivität

journal.window.content.body.attribution = wie { $source } es aufgezeichnet hat

journal.window.content.body.attribution.from = wie { $source } es aufgezeichnet hat, von { $by }

journal.window.content.body.attribution.values = { $attribution }: { $values }

journal.window.content.body.heading = ## { $source } { $kind }: { $label }

journal.window.content.body.kind.default = Erlebnis

journal.window.content.body.span = { $start } bis { $end }

journal.window.content.browser.app.paused = solstone-App pausiert

journal.window.content.browser.context = Kontext: { $details }

journal.window.content.browser.duration.d = { $days } d

journal.window.content.browser.duration.dh = { $days } d { $hours } h

journal.window.content.browser.duration.h = { $hours } h

journal.window.content.browser.duration.hm = { $hours } h { $minutes } min

journal.window.content.browser.duration.m = { $minutes } min

journal.window.content.browser.duration.s = { $seconds } s

journal.window.content.browser.language = Sprache { $lang }

journal.window.content.browser.media.playing = Medien werden abgespielt

journal.window.content.browser.media.playing.muted = Medien werden wiedergegeben, stummgeschaltet

journal.window.content.browser.page.default = Browserseite

journal.window.content.browser.tab.age = Tab seit mindestens { $duration } offen

journal.window.content.browser.tab.background = Tab im Hintergrund

journal.window.content.browser.tab.front = vorderer Tab

journal.window.content.browser.tab.front.focused = Tab im Vordergrund, Fenster fokussiert

journal.window.content.browser.tab.front.unfocused = vorderer Tab, Fenster nicht fokussiert

journal.window.content.chat.speaker.assistant = Assistent

journal.window.content.chat.speaker.human = Mensch

journal.window.content.chat.speaker.unknown = Unbekannt

journal.window.content.chat.speaker.you = Du

journal.window.content.chat.turn = **{ $speaker }:** { $text }

journal.window.content.desktop.activity.field.app = App: { $value }

journal.window.content.desktop.activity.field.app.id = app_id: { $value }

journal.window.content.desktop.activity.field.capability = Fähigkeit: { $value }

journal.window.content.desktop.activity.field.cwd = cwd: { $value }

journal.window.content.desktop.activity.field.end = end_ms: { $value }

journal.window.content.desktop.activity.field.exe = Programmdatei: { $value }

journal.window.content.desktop.activity.field.kind = Art: { $value }

journal.window.content.desktop.activity.field.method = Methode: { $value }

journal.window.content.desktop.activity.field.pane.title = pane_title: { $value }

journal.window.content.desktop.activity.field.path = Pfad: { $value }

journal.window.content.desktop.activity.field.session = Sitzung: { $value }

journal.window.content.desktop.activity.field.start = start_ms: { $value }

journal.window.content.desktop.activity.field.state = Zustand: { $value }

journal.window.content.desktop.activity.field.status = Status: { $value }

journal.window.content.desktop.activity.field.stream = Datenstrom: { $value }

journal.window.content.desktop.activity.field.title = Titel: { $value }

journal.window.content.desktop.activity.field.url = URL: { $value }

journal.window.content.desktop.activity.field.window = Fenster: { $value }

journal.window.content.desktop.activity.heading = Schreibtischaktivität

journal.window.content.document.absent = Nicht in diesem Dokument angegeben

journal.window.content.document.asset.unnamed = Unbenanntes Asset

journal.window.content.document.detail.applies = Gilt für: { $value }

journal.window.content.document.detail.timing = Zeitangabe: { $value }

journal.window.content.document.party.unnamed = Unbenannte Partei

journal.window.content.document.section.assets = Vermögenswerte und Eigentum

journal.window.content.document.section.conditions = Bedingungen und Auslöser

journal.window.content.document.section.dates = Wichtige Termine

journal.window.content.document.section.overview = Überblick

journal.window.content.document.section.parties = Parteien und Rollen

journal.window.content.document.section.provisions = Wesentliche Bestimmungen

journal.window.content.document.section.summary = Zusammenfassung

journal.window.content.entity.aka = **Auch bekannt als:** { $items }

journal.window.content.entity.description.none = *(Keine Beschreibung verfügbar)*

journal.window.content.entity.name.unnamed = Unbenannt

journal.window.content.entity.tags = **Schlagwörter:** { $items }

journal.window.content.entity.type.unknown = Unbekannt

journal.window.content.event.created = **Erstellt am:** { $date }

journal.window.content.event.heading.occurred = ### { $kind }: { $title }

journal.window.content.event.heading.planned = ### Geplant { $kind }: { $title }

journal.window.content.event.participants.occurred = **Teilnehmende:** { $names }

journal.window.content.event.participants.planned = **Erwartete Personen:** { $names }

journal.window.content.event.time.occurred = **Zeitpunkt:** { $start }

journal.window.content.event.time.occurred.range = **Zeitpunkt:** { $start } - { $end }

journal.window.content.event.time.scheduled = **Geplante Zeit:** { $start }

journal.window.content.event.time.scheduled.range = **Geplante Zeit:** { $start } - { $end }

journal.window.content.event.type.default = Ereignis

journal.window.content.import.attendees = Teilnehmende: { $attendees }

journal.window.content.import.book.by = ## { $book } von { $author }

journal.window.content.import.book.unknown = Unbekanntes Buch

journal.window.content.import.duration.min = { $minutes } min

journal.window.content.import.event.untitled = Unbenannter Termin

journal.window.content.import.links = Links: { $links }

journal.window.content.import.loc = Ort { $location }

journal.window.content.import.location = Ort: { $location }

journal.window.content.import.note.line = Notiz: { $content }

journal.window.content.import.note.untitled = Unbetitelte Notiz

journal.window.content.import.page = Seite { $page }

journal.window.content.import.tags = Tags: { $tags }

journal.window.content.objects.class.bird = Vogel

journal.window.content.objects.class.bottle = Flasche

journal.window.content.objects.class.bowl = Schüssel

journal.window.content.objects.class.car = Auto

journal.window.content.objects.class.cell.phone = Handy

journal.window.content.objects.class.cup = Tasse

journal.window.content.objects.class.laptop = Laptop

journal.window.content.objects.class.person = Person

journal.window.content.objects.class.tie = Krawatte

journal.window.content.objects.class.truck = Lastwagen

journal.window.content.objects.class.tv = Fernseher

journal.window.content.objects.count = { $name } ×{ $count }

journal.window.content.objects.line = **Objekte:** { $summary }

journal.window.content.observation.observed = - { $content } (beobachtet: { $day })

journal.window.content.possible.name = möglicher Name: { $name }

journal.window.content.protected.video.body = Ein Fenster von { $service } war etwa { $duration } lang auf dem Bildschirm, während die Annahme (Bildschirm und Audio) pausiert war.

journal.window.content.protected.video.body.app = Ein { $service }-Fenster war in { $app } etwa { $duration } lang auf dem Bildschirm, während die Annahme (Bildschirm und Audio) pausiert war.

journal.window.content.protected.video.body.app.title = Ein { $service }-Fenster war auf dem Bildschirm in { $app }, mit dem Titel "{ $title }", etwa { $duration } lang, während die Annahme (Bildschirm und Audio) pausiert war.

journal.window.content.protected.video.body.title = Ein Fenster von { $service } war auf dem Bildschirm, mit dem Titel "{ $title }", etwa { $duration } lang, während die Annahme (Bildschirm und Audio) pausiert war.

journal.window.content.protected.video.heading = ## Geschütztes Video: { $service }

journal.window.content.protected.video.minutes = {
    $quantity ->
        [one] 1 Minute
       *[other] { $quantity } Minuten
}

journal.window.content.raw.screen.category = **Kategorie:** { $category }

journal.window.content.raw.screen.meeting = **Besprechung** ({ $platform })

journal.window.content.raw.screen.participants = **Teilnehmende:**

journal.window.content.raw.screen.share = **Bildschirmfreigabe:**

journal.window.content.raw.screen.share.by = **Bildschirmfreigabe von { $presenter }:**

journal.window.content.raw.screen.unknown = unbekannt

journal.window.content.raw.screen.unknown.title = Unbekannt

journal.window.content.screen.entities.heading = ## Entitäten

journal.window.content.screen.entities.none = Nicht angegeben

journal.window.content.screen.entity.default = Entität

journal.window.content.workout.calories = { $kcal } kcal

journal.window.content.workout.distance = { $km } km

journal.window.content.workout.duration.h = { $hours } h

journal.window.content.workout.duration.hm = { $hours } h { $minutes } min

journal.window.content.workout.duration.m = { $minutes } min

journal.window.content.workout.elapsed = { $duration } vergangen

journal.window.content.workout.elevation = Höhengewinn { $m } m

journal.window.content.workout.flag.commute = Arbeitsweg

journal.window.content.workout.flag.manual = von Hand eingetragen

journal.window.content.workout.heading = ## Strava-Training: { $name }

journal.window.content.workout.hr.avg = durchschnittlicher Puls { $avg } bpm

journal.window.content.workout.hr.avg.max = durchschnittlicher Puls { $avg } bpm, max. { $max } bpm

journal.window.content.workout.hr.max = Puls max { $max } bpm

journal.window.content.workout.more = mehr von Strava:

journal.window.content.workout.moving = { $duration } in Bewegung

journal.window.content.workout.name.default = Training

journal.window.content.workout.power.avg = mittlere Leistung { $avg } W

journal.window.content.workout.power.avg.weighted = Leistungsschnitt { $avg } W, gewichtet { $weighted } W

journal.window.content.workout.power.weighted = gewichtete Leistung { $weighted } W

journal.window.content.workout.recorded = wie Strava es aufgezeichnet hat: { $details }

journal.window.content.workout.started = gestartet { $time }

journal.window.home.accessible.name.days.of.the.week = Wochentage

journal.window.home.accessible.name.originals.pages = Seiten der Originale

journal.window.home.accessible.name.which.day = welcher Tag

journal.window.home.account.tab.today = heute

journal.window.home.account.tab.yesterday = gestern

journal.window.home.action.all.of.todays.activities = alle Aktivitäten von heute →

journal.window.home.action.check.status = Status prüfen

journal.window.home.action.copy = kopieren

journal.window.home.action.how.this.was.written = wie das geschrieben wurde →

journal.window.home.action.open.it = es öffnen →

journal.window.home.action.open.your.week = deine Woche öffnen →

journal.window.home.action.see.health = Zustand ansehen →

journal.window.home.action.see.the.run = den Lauf ansehen →

journal.window.home.action.undo = rückgängig machen

journal.window.home.action.what.it.fixes = was es behebt ↗

journal.window.home.activity.source.browser = aus deinem Browser

journal.window.home.activity.source.browser.named = von deinem Browser ({ $device })

journal.window.home.activity.source.computer = von deinem Computer

journal.window.home.activity.source.computer.named = von deinem Computer ({ $device })

journal.window.home.activity.source.ipad = von deinem iPad

journal.window.home.activity.source.ipad.named = aus deinem iPad ({ $device })

journal.window.home.activity.source.iphone = von deinem iPhone

journal.window.home.activity.source.iphone.named = von deinem iPhone ({ $device })

journal.window.home.activity.source.mac = von deinem Mac

journal.window.home.activity.source.mac.named = von deinem Mac ({ $device })

journal.window.home.activity.source.phone = von deinem Telefon

journal.window.home.activity.source.phone.named = aus deinem Telefon ({ $device })

journal.window.home.activity.source.phrase = von deinem{ $slot1 }{ $kind }

journal.window.home.activity.source.phrase.named = von deinem{ $slot1 }{ $kind }{ $slot2 }({ $device })

journal.window.home.activity.source.tablet = von deinem Tablet

journal.window.home.activity.source.tablet.named = von deinem Tablet ({ $device })

journal.window.home.activity.source.terminal = von deinem Terminal

journal.window.home.activity.source.terminal.named = von deinem Terminal ({ $device })

journal.window.home.activity.source.watch = von deiner Uhr

journal.window.home.activity.source.watch.named = von deiner Uhr ({ $device })

journal.window.home.activity.title.fallback = unbenannte Aktivität

journal.window.home.attention.agent.errors.count = { $quantity ->
        [one] { $count } Fehler eines Agenten heute. frag, was passiert ist
       *[other] { $count } Fehler von Agenten heute. frag, was passiert ist
    }

journal.window.home.attention.heading = wartet auf dich

journal.window.home.attention.import.complete = Import abgeschlossen.

journal.window.home.body.account.mindfulness = Achtsamkeit

journal.window.home.body.account.sleep = Schlaf

journal.window.home.body.activity.line = { $arrangement ->
       *[untimed-no-distance-no-source-intact] { $activity }.
        [untimed-no-distance-no-source-part-removed] { $activity }, ein Teil davon entfernt.
        [untimed-no-distance-logged-intact] { $activity }, von dir protokolliert.
        [untimed-no-distance-logged-part-removed] { $activity }, von dir protokolliert, ein Teil davon entfernt.
        [untimed-no-distance-recorded-intact] { $activity }, von { $source_names }.
        [untimed-no-distance-recorded-part-removed] { $activity }, von { $source_names }, ein Teil davon entfernt.
        [untimed-distance-no-source-intact] { $activity }, { $distance }.
        [untimed-distance-no-source-part-removed] { $activity }, { $distance }, ein Teil davon entfernt.
        [untimed-distance-logged-intact] { $activity }, { $distance }, von dir protokolliert.
        [untimed-distance-logged-part-removed] { $activity }, { $distance }, von dir protokolliert, ein Teil davon entfernt.
        [untimed-distance-recorded-intact] { $activity }, { $distance }, von { $source_names }.
        [untimed-distance-recorded-part-removed] { $activity }, { $distance }, von { $source_names }, ein Teil davon entfernt.
        [start-no-distance-no-source-intact] { $activity }, ab { $started_at }.
        [start-no-distance-no-source-part-removed] { $activity }, ab { $started_at }, ein Teil davon entfernt.
        [start-no-distance-logged-intact] { $activity }, ab { $started_at }, von dir protokolliert.
        [start-no-distance-logged-part-removed] { $activity }, ab { $started_at }, von dir protokolliert, ein Teil davon entfernt.
        [start-no-distance-recorded-intact] { $activity }, ab { $started_at }, von { $source_names }.
        [start-no-distance-recorded-part-removed] { $activity }, ab { $started_at }, von { $source_names }, ein Teil davon entfernt.
        [start-distance-no-source-intact] { $activity }, ab { $started_at }, { $distance }.
        [start-distance-no-source-part-removed] { $activity }, ab { $started_at }, { $distance }, ein Teil davon entfernt.
        [start-distance-logged-intact] { $activity }, ab { $started_at }, { $distance }, von dir protokolliert.
        [start-distance-logged-part-removed] { $activity }, ab { $started_at }, { $distance }, von dir protokolliert, ein Teil davon entfernt.
        [start-distance-recorded-intact] { $activity }, ab { $started_at }, { $distance }, von { $source_names }.
        [start-distance-recorded-part-removed] { $activity }, ab { $started_at }, { $distance }, von { $source_names }, ein Teil davon entfernt.
        [range-no-distance-no-source-intact] { $activity }, { $started_at } bis { $ended_at }.
        [range-no-distance-no-source-part-removed] { $activity }, { $started_at } bis { $ended_at }, ein Teil davon entfernt.
        [range-no-distance-logged-intact] { $activity }, { $started_at } bis { $ended_at }, von dir protokolliert.
        [range-no-distance-logged-part-removed] { $activity }, { $started_at } bis { $ended_at }, von dir protokolliert, ein Teil davon entfernt.
        [range-no-distance-recorded-intact] { $activity }, { $started_at } bis { $ended_at }, von { $source_names }.
        [range-no-distance-recorded-part-removed] { $activity }, { $started_at } bis { $ended_at }, von { $source_names }, ein Teil davon entfernt.
        [range-distance-no-source-intact] { $activity }, { $started_at } bis { $ended_at }, { $distance }.
        [range-distance-no-source-part-removed] { $activity }, { $started_at } bis { $ended_at }, { $distance }, ein Teil davon entfernt.
        [range-distance-logged-intact] { $activity }, { $started_at } bis { $ended_at }, { $distance }, von dir protokolliert.
        [range-distance-logged-part-removed] { $activity }, { $started_at } bis { $ended_at }, { $distance }, von dir protokolliert, ein Teil davon entfernt.
        [range-distance-recorded-intact] { $activity }, { $started_at } bis { $ended_at }, { $distance }, von { $source_names }.
        [range-distance-recorded-part-removed] { $activity }, { $started_at } bis { $ended_at }, { $distance }, von { $source_names }, ein Teil davon entfernt.
    }

journal.window.home.body.sources.and = { $start }{ $slot1 }und{ $slot2 }{ $last }

journal.window.home.body.time.with.day = { $time }{ $slot1 }am{ $slot2 }{ $day }

journal.window.home.briefing.expected.time = gestern ist gewöhnlich bis 10 Uhr vormittags aufgeschrieben.

journal.window.home.briefing.late.hour.count = { $quantity ->
       [one] es ist { $hours } h zu spät.
      *[other] es ist { $hours } h zu spät.
    }

journal.window.home.briefing.written.timestamp = geschrieben um { $written_at }

journal.window.home.button.label.show.all = alle anzeigen →

journal.window.home.button.label.try.again = Erneut versuchen

journal.window.home.connections.events.count = { $quantity ->
        [one] 1 Ereignis
       *[other] { $count } Ereignisse
    }

journal.window.home.connections.evidence.transcription = transkribiert als "{ $label }"

journal.window.home.connections.horizon.day = Verbindungen aus gemeinsamer Zeit am selben Ort reichen nur bis { $day } zurück.

journal.window.home.connections.horizon.earlier.days = { $quantity ->
        [one] ein früherer Tag ist noch in deinem Journal; diese Verbindungen wurden noch nicht aufgebaut.
       *[other] die { $earlier_days } früheren Tage sind noch in deinem Journal; diese Verbindungen wurden noch nicht aufgebaut.
    }

journal.window.home.connections.mentions.count = { $quantity ->
        [one] 1 Name in deinem Journal erwähnt
       *[other] { $count } Namen in deinem Journal erwähnt
    }

journal.window.home.connections.moments.count = { $quantity ->
        [one] 1 Moment
       *[other] { $count } Momente
    }

journal.window.home.connections.moments.recent.count = { $quantity ->
        [one] 1 Moment · zuletzt { $day }
       *[other] { $count } Momente · zuletzt { $day }
    }

journal.window.home.device.last.added.day = { $quantity ->
       [one] die solstone-App auf { $name } hat zuletzt vor { $count } Tag etwas zu deinem Journal hinzugefügt.
      *[other] die solstone-App auf { $name } hat zuletzt vor { $count } Tagen etwas zu deinem Journal hinzugefügt.
    }

journal.window.home.device.last.added.hour = { $quantity ->
       [one] die solstone-App auf { $name } hat zuletzt vor { $count } Stunde etwas zu deinem Journal hinzugefügt.
      *[other] die solstone-App auf { $name } hat zuletzt vor { $count } Stunden etwas zu deinem Journal hinzugefügt.
    }

journal.window.home.device.last.added.minute = { $quantity ->
       [one] die solstone-App auf { $name } hat deinem Journal zuletzt vor { $count } Minute etwas hinzugefügt.
      *[other] die solstone-App auf { $name } hat deinem Journal zuletzt vor { $count } Minuten etwas hinzugefügt.
    }

journal.window.home.device.last.added.month = { $quantity ->
       [one] die solstone-App auf { $name } hat deinem Journal zuletzt vor { $count } Monat etwas hinzugefügt.
      *[other] die solstone-App auf { $name } hat deinem Journal zuletzt vor { $count } Monaten etwas hinzugefügt.
    }

journal.window.home.device.last.added.second = { $quantity ->
       [one] die solstone-App auf { $name } hat zuletzt vor { $count } Sekunde etwas zu deinem Journal hinzugefügt.
      *[other] die solstone-App auf { $name } hat zuletzt vor { $count } Sekunden etwas zu deinem Journal hinzugefügt.
    }

journal.window.home.device.last.added.week = { $quantity ->
       [one] die solstone-App auf { $name } hat deinem Journal zuletzt vor { $count } Woche etwas hinzugefügt.
      *[other] die solstone-App auf { $name } hat deinem Journal zuletzt vor { $count } Wochen etwas hinzugefügt.
    }

journal.window.home.device.paired = die solstone-App auf{ $slot1 }{ $name }{ $slot2 }ist gekoppelt.

journal.window.home.device.paired.awaiting = die solstone-App auf{ $slot1 }{ $name }{ $slot2 }ist gekoppelt. was du mit ihr teilst, kommt etwa alle 5 Minuten in deinem Journal an.

journal.window.home.disclosure.label.review.originals = Originale prüfen

journal.window.home.disclosure.label.what.this.summary.covers = was diese Zusammenfassung abdeckt

journal.window.home.download.speech.progress = Modell für Gesprochenes wird heruntergeladen · { $downloaded_gb } GB von { $total_gb } GB

journal.window.home.download.thinking.progress = lokales Modell wird heruntergeladen · { $downloaded_gb } GB von { $total_gb } GB

journal.window.home.first.segment.arrived = dein erstes Segment, von{ $slot1 }{ $time }, ist angekommen.

journal.window.home.first.segment.pending = dein erstes Segment, von{ $slot1 }{ $time }, ist angekommen. Denken hat noch nicht damit aufgeholt.

journal.window.home.first.segment.read = dein erstes Segment, von{ $slot1 }{ $time }, ist angekommen, und Denken hat es gelesen.

journal.window.home.health.age.just.now = gerade eben

journal.window.home.health.attention.count = { $quantity ->
        [one] 1 Sache braucht deine Aufmerksamkeit
       *[other] { $count } Sachen brauchen deine Aufmerksamkeit
    }

journal.window.home.health.awaiting.first.headline = die solstone-App auf einem deiner Geräte hat deinem Journal noch nichts hinzugefügt.

journal.window.home.health.backlog.day.needs.help = ein Tag im Journal braucht Hilfe.

journal.window.home.health.device.age = { $device } ({ $age })

journal.window.home.health.device.setup = eins einrichten →

journal.window.home.health.device.sources = { $device } ({ $sources })

journal.window.home.health.devices.view = Geräte anzeigen →

journal.window.home.health.empty.registry.headline = noch läuft die solstone-App auf keinem Gerät. richte eins ein, um dein Journal zu starten.

journal.window.home.health.no.eligible.headline = auf keinem deiner Geräte ist die solstone-App gerade eingerichtet, um deinem Journal etwas hinzuzufügen.

journal.window.home.health.not.yet.active.headline = die solstone-App fügt deinem Journal hinzu.

journal.window.home.health.quiet.devices = { $quantity ->
        [one] { $count } Gerät hat deinem Journal in letzter Zeit nichts hinzugefügt: { $devices }.
       *[other] { $count } Geräte haben deinem Journal in letzter Zeit nichts hinzugefügt: { $devices }.
    }

journal.window.home.health.quiet.elapsed.day = { $quantity ->
        [one] { $device } hat deinem Journal seit { $count } Tag nichts hinzugefügt.
       *[other] { $device } hat deinem Journal seit { $count } Tagen nichts hinzugefügt.
    }

journal.window.home.health.quiet.elapsed.hour = { $quantity ->
        [one] { $device } hat seit { $count } Stunde nichts zu deinem Journal hinzugefügt.
       *[other] { $device } hat seit { $count } Stunden nichts zu deinem Journal hinzugefügt.
    }

journal.window.home.health.quiet.elapsed.just.now = { $device } hat deinem Journal zuletzt gerade eben etwas hinzugefügt.

journal.window.home.health.quiet.elapsed.minute = { $quantity ->
        [one] { $device } hat deinem Journal seit { $count } Minute nichts hinzugefügt.
       *[other] { $device } hat deinem Journal seit { $count } Minuten nichts hinzugefügt.
    }

journal.window.home.health.quiet.elapsed.month = { $quantity ->
        [one] { $device } hat seit { $count } Monat nichts zu deinem Journal hinzugefügt.
       *[other] { $device } hat seit { $count } Monaten nichts zu deinem Journal hinzugefügt.
    }

journal.window.home.health.quiet.elapsed.second = { $quantity ->
        [one] { $device } hat seit { $count } Sekunde nichts zu deinem Journal hinzugefügt.
       *[other] { $device } hat seit { $count } Sekunden nichts zu deinem Journal hinzugefügt.
    }

journal.window.home.health.quiet.elapsed.week = { $quantity ->
        [one] { $device } hat seit { $count } Woche nichts zu deinem Journal hinzugefügt.
       *[other] { $device } hat seit { $count } Wochen nichts zu deinem Journal hinzugefügt.
    }

journal.window.home.health.quiet.one.unknown = { $device }{ $slot1 }hat in letzter Zeit nichts zu deinem Journal hinzugefügt.

journal.window.home.health.ready = alles funktioniert

journal.window.home.health.residue.headline = gerade braucht nichts deine Aufmerksamkeit.

journal.window.home.health.unavailable.headline = der Status deiner Geräte ist gerade unklar.

journal.window.home.health.unnamed.idle.issue = die solstone-App auf einem deiner Geräte hat in letzter Zeit nichts zu deinem Journal hinzugefügt.

journal.window.home.health.upload.devices = { $quantity ->
        [one] dein Journal konnte Uploads von { $devices } nicht annehmen.
       *[other] dein Journal konnte Uploads von { $count } Geräten nicht annehmen: { $devices }.
    }

journal.window.home.held.activity.safety.check = die Verarbeitung der Aktivität ist für eine Sicherheitsprüfung ausgesetzt.

journal.window.home.held.all.safety.check = alle Arbeit in der Warteschlange ist für eine Sicherheitsprüfung angehalten.

journal.window.home.held.daily.safety.check = die tägliche Verarbeitung ist für eine Sicherheitsprüfung zurückgehalten.

journal.window.home.held.flush.safety.check = das Abschließen von Aktivitäten ist für eine Sicherheitsprüfung zurückgestellt.

journal.window.home.held.queued.safety.check = Verarbeitung in der Warteschlange ist für eine Sicherheitsprüfung zurückgehalten.

journal.window.home.held.search.safety.check = Suchaktualisierungen sind für eine Sicherheitsprüfung ausgesetzt.

journal.window.home.held.segment.safety.check = die Verarbeitung des Segments ist für eine Sicherheitsprüfung zurückgehalten.

journal.window.home.held.weekly.safety.check = die wöchentliche Rückschau ist für eine Sicherheitsprüfung zurückgehalten.

journal.window.home.link.label.all.connections = alle Verbindungen →

journal.window.home.link.label.cancel.on.the.thinking.page = abbrechen auf der Seite Denken →

journal.window.home.link.label.check.system.health = Systemzustand prüfen →

journal.window.home.link.label.health = Zustand →

journal.window.home.link.label.name.voices.in.speakers = Stimmen in Sprechern benennen →

journal.window.home.link.label.other.agents.and.pairing.codes = andere Agenten und Kopplungscodes →

journal.window.home.link.label.pair.a.device = Gerät koppeln →

journal.window.home.link.label.try.again.on.the.thinking.page = auf der Seite Denken erneut versuchen →

journal.window.home.narrative.coverage.batch.counts = { $segment_quantity ->
    [one] basierend auf { $processed_segments } von { $batch_segments } verarbeitetem Segment und { $processed_activities } von { $batch_activities } { $activity_quantity ->
        [one] Aktivitätseintrag
       *[other] Aktivitätseinträgen
    } in diesem Stapel.
   *[other] basierend auf { $processed_segments } von { $batch_segments } verarbeiteten Segmenten und { $processed_activities } von { $batch_activities } { $activity_quantity ->
        [one] Aktivitätseintrag
       *[other] Aktivitätseinträgen
    } in diesem Stapel.
}

journal.window.home.narrative.coverage.clock = das ist Verarbeitungszeit, nicht wann die Aktivität passiert ist.

journal.window.home.narrative.coverage.overlap = Einträge können sich überschneiden; das ist keine Zusammenfassung des ganzen Tages.

journal.window.home.narrative.coverage.started = das Verarbeitungsfenster beginnt um { $start_time }.

journal.window.home.narrative.updated.timestamp = aktualisiert um { $updated_at }

journal.window.home.needs.device.one = { $name }{ $slot1 }erreicht dein Journal nicht.

journal.window.home.needs.devices.named = { $quantity ->
        [one] { $count } Gerät erreicht dein Journal nicht: { $names }.
       *[other] { $count } Geräte erreichen dein Journal nicht: { $names }.
    }

journal.window.home.needs.devices.unnamed = { $quantity ->
        [one] { $count } Gerät erreicht dein Journal nicht.
       *[other] { $count } Geräte erreichen dein Journal nicht.
    }

journal.window.home.needs.devices.unnamed.count = { $quantity ->
        [one] { $count } unbenanntes Gerät
       *[other] { $count } unbenannte Geräte
    }

journal.window.home.needs.sources.failing = die solstone-App auf einem deiner Geräte hat Mühe beim Hinzufügen von{ $slot1 }{ $sources }{ $slot2 }zu deinem Journal.

journal.window.home.needs.voice.candidate.ready = dein Journal hat deine Stimme vielleicht gefunden. spiel ein paar Stimmproben ab und sag, ob du es bist.

journal.window.home.needs.voice.damaged = deine gespeicherte Stimme kann nicht gelesen werden. entferne sie auf der Seite Sprecher, dann lern sie erneut an.

journal.window.home.needs.voice.declined.template = deine Stimme ist noch nicht eingerichtet. dein Journal schaut nach{ $slot1 }{ $date } erneut, oder du kannst sie jetzt anlernen.

journal.window.home.needs.voice.needs.more.audio = deine Stimme ist noch nicht eingerichtet. dein Journal sucht sie jeden Tag, oder du kannst sie jetzt anlernen.

journal.window.home.needs.voice.next.look.date = { $weekday }{ $slot1 }{ $day }.{ $slot2 }{ $month }

journal.window.home.needs.voice.too.short.or.mixed = deine Stimme ist noch nicht eingerichtet. bisher sind deine Äußerungen meist kurz oder mit anderen Stimmen gemischt, Anlernen hilft deshalb am meisten.

journal.window.home.needs.voice.unusable = deine Stimme lässt sich aus dem Vorhandenen nicht einrichten. lehre sie auf der Seite Sprecher.

journal.window.home.night.briefing.not.written = gestern wurde nicht aufgeschrieben.

journal.window.home.night.gap.additional = { $quantity ->
        [one] …und { $count } weitere wurden nicht fertig.
       *[other] …und { $count } weitere wurden nicht fertig.
    }

journal.window.home.night.gap.runs.failed = { $quantity ->
        [one] der { $label }-Lauf wurde nicht fertig.
       *[other] { $count } { $label }-Läufe wurden nicht fertig.
    }

journal.window.home.night.gap.runs.failed.briefing = { $quantity ->
        [one] der tägliche Briefing-Lauf wurde nicht abgeschlossen.
       *[other] { $count } tägliche Briefing-Läufe wurden nicht abgeschlossen.
    }

journal.window.home.night.gap.runs.failed.documents = { $quantity ->
        [one] der Dokumentlauf wurde nicht fertig.
       *[other] { $count } Dokumentläufe wurden nicht fertig.
    }

journal.window.home.night.gap.runs.lost = { $quantity ->
        [one] der { $label }-Lauf konnte nicht starten.
       *[other] { $count } { $label }-Läufe konnten nicht starten.
    }

journal.window.home.night.gap.runs.lost.briefing = { $quantity ->
        [one] der tägliche Briefing-Lauf konnte nicht starten.
       *[other] { $count } tägliche Briefing-Läufe konnten nicht starten.
    }

journal.window.home.night.gap.runs.lost.documents = { $quantity ->
        [one] der Dokument-Lauf konnte nicht starten.
       *[other] { $count } Dokument-Läufe konnten nicht starten.
    }

journal.window.home.night.notes.not.written = die Notizen von gestern wurden nicht alle geschrieben.

journal.window.home.night.processing.did.not.finish = ein Teil der Verarbeitung über Nacht wurde nicht fertig.

journal.window.home.night.review.did.not.finish = die Prüfung über Nacht wurde nicht fertig.

journal.window.home.night.status.unavailable = der Verarbeitungsstatus von gestern ist nicht verfügbar.

journal.window.home.night.work.did.not.finish = manche Arbeit über Nacht wurde nicht fertig.

journal.window.home.originals.bulk.limit = wähle bis zu { $selection_limit } Elemente auf einmal. nichts wurde gelöscht.

journal.window.home.originals.bulk.selection.count = { $quantity ->
    [one] 1 ausgewählt
   *[other] { $selected_count } ausgewählt
    }

journal.window.home.originals.bulk.selection.over.cap = { $selected_count } ausgewählt. eine Aktion umfasst bis zu { $selection_limit }.

journal.window.home.originals.card.page.total = { $quantity ->
    [one] auf dieser Seite: 1 Original · { $size }
   *[other] auf dieser Seite: { $original_count } Originale · { $size }
    }

journal.window.home.originals.confirm.bulk.delete.first = das löscht { $original_count } Originale.

journal.window.home.originals.confirm.bulk.delete.third = sonst geht nichts mit ihnen.

journal.window.home.originals.confirm.bulk.trash.third = sie warten im Papierkorb, dann löscht der Papierkorb sie endgültig und ihr Speicherplatz kommt zurück.

journal.window.home.originals.confirm.bulk.trash.until.erase.first = das verschiebt { $original_count } Originale in den Papierkorb.

journal.window.home.originals.confirm.bulk.trash.until.erase.fourth = du kannst sie bis dahin wiederherstellen.

journal.window.home.originals.confirm.bulk.trash.until.erase.third = { $days_quantity ->
    [one] der Papierkorb löscht sie nach 1 Tag endgültig, und ihr Speicherplatz kommt dann zurück.
   *[other] der Papierkorb löscht sie nach { $retention_days } Tagen endgültig, und ihr Speicherplatz kommt dann zurück.
    }

journal.window.home.originals.confirm.delete.action = { $quantity ->
    [one] es löschen
   *[other] sie löschen
    }

journal.window.home.originals.confirm.delete.default.stream.first = { $quantity ->
    [one] das löscht 1 Original von { $date_and_time }.
   *[other] das löscht { $original_count } Originale von { $date_and_time }.
    }

journal.window.home.originals.confirm.delete.heading = { $quantity ->
    [one] dieses Original löschen?
   *[other] diese Originale löschen?
    }

journal.window.home.originals.confirm.delete.named.stream.first = { $quantity ->
    [one] dies löscht 1 Original von { $date_and_time } · { $stream_name }.
   *[other] dies löscht { $original_count } Originale von { $date_and_time } · { $stream_name }.
    }

journal.window.home.originals.confirm.delete.named.stream.fourth = das lässt sich nicht rückgängig machen.

journal.window.home.originals.confirm.delete.named.stream.second = deine Sicherung hat eine Kopie, die von hier nicht geprüft werden kann.

journal.window.home.originals.confirm.delete.named.stream.third = { $quantity ->
    [one] es geht nichts anderes damit.
   *[other] es geht nichts anderes mit ihnen.
    }

journal.window.home.originals.confirm.mixed.delete.first = { $quantity ->
    [one] deine Sicherung hat das eine, das gelöscht wird, von deinem Gerät kopiert.
   *[other] deine Sicherung hat diejenigen, die gelöscht werden, von deinem Gerät kopiert.
    }

journal.window.home.originals.confirm.mixed.delete.third = { $quantity ->
    [one] es zu löschen lässt sich nicht rückgängig machen.
   *[other] sie zu löschen lässt sich nicht rückgängig machen.
    }

journal.window.home.originals.confirm.mixed.summary = das löscht { $deleted_count } dieser Originale und verschiebt { $trashed_count } in den Papierkorb. nichts anderes geht mit.

journal.window.home.originals.confirm.mixed.trash.first = { $quantity ->
    [one] das eine, das in den Papierkorb verschoben wird, wartet dort, dann löscht der Papierkorb es endgültig und sein Platz kommt zurück.
   *[other] die, die in den Papierkorb verschoben werden, warten dort, dann löscht der Papierkorb sie endgültig und ihr Platz kommt zurück.
    }

journal.window.home.originals.confirm.mixed.trash.until.erase.first = { $quantity ->
    [one] das eine, das in den Papierkorb verschoben wird, wird nach { $days_quantity ->
    [one] 1 Tag
   *[other] { $retention_days } Tagen
    } endgültig gelöscht, und sein Speicherplatz kommt dann zurück.
   *[other] die, die in den Papierkorb verschoben werden, werden nach { $days_quantity ->
    [one] 1 Tag
   *[other] { $retention_days } Tagen
    } endgültig gelöscht, und ihr Speicherplatz kommt dann zurück.
    }

journal.window.home.originals.confirm.trash.named.stream.third = { $quantity ->
    [one] es wartet im Papierkorb, dann löscht der Papierkorb es endgültig und sein Speicherplatz kommt zurück.
   *[other] sie warten im Papierkorb, dann löscht der Papierkorb sie endgültig und ihr Speicherplatz kommt zurück.
    }

journal.window.home.originals.confirm.trash.until.erase.default.stream.first = { $quantity ->
    [one] das verschiebt 1 Original von { $date_and_time } in den Papierkorb.
   *[other] das verschiebt { $original_count } Originale von { $date_and_time } in den Papierkorb.
    }

journal.window.home.originals.confirm.trash.until.erase.named.stream.first = { $quantity ->
    [one] das verschiebt 1 Original von { $date_and_time } · { $stream_name } in den Papierkorb.
   *[other] das verschiebt { $original_count } Originale von { $date_and_time } · { $stream_name } in den Papierkorb.
    }

journal.window.home.originals.confirm.trash.until.erase.named.stream.fourth = { $quantity ->
    [one] du kannst es bis dahin wiederherstellen.
   *[other] du kannst sie bis dahin wiederherstellen.
    }

journal.window.home.originals.confirm.trash.until.erase.named.stream.third = { $quantity ->
    [one] der Papierkorb löscht es nach { $days_quantity ->
    [one] 1 Tag
   *[other] { $retention_days } Tagen
    } endgültig, und sein Speicherplatz kommt dann zurück.
   *[other] der Papierkorb löscht sie nach { $days_quantity ->
    [one] 1 Tag
   *[other] { $retention_days } Tagen
    } endgültig, und ihr Speicherplatz kommt dann zurück.
    }

journal.window.home.originals.copy.bulk.clear = Auswahl aufheben

journal.window.home.originals.copy.bulk.delete = Ausgewähltes löschen

journal.window.home.originals.copy.bulk.keep = Auswahl behalten

journal.window.home.originals.copy.bulk.select.all = diese Seite auswählen

journal.window.home.originals.copy.card.empty = nichts wartet auf dich.

journal.window.home.originals.copy.card.heading = Originale, die auf dich warten

journal.window.home.originals.copy.card.loading = Originale werden geladen…

journal.window.home.originals.copy.card.subhead = Audio- und Video-Originale, zum Löschen markiert, plus alles, was eine Löschung unfertig gelassen hat. nichts, was hier markiert ist, wird gelöscht, bis du es sagst.

journal.window.home.originals.copy.card.support = den Support um Hilfe bitten

journal.window.home.originals.copy.card.too.large = die Löschdetails dieses Elements sind zu groß, um sie hier zu prüfen.

journal.window.home.originals.copy.card.unavailable = die Liste konnte nicht geladen werden.

journal.window.home.originals.copy.confirm.cancel = abbrechen

journal.window.home.originals.copy.confirm.go.trash = in den Papierkorb verschieben

journal.window.home.originals.copy.confirm.recover.body = das schließt jede Löschung ab, die mittendrin anhielt. jedes noch nicht gelöschte Original kehrt in sein Segment zurück. der Rest bleibt gelöscht, und was deine Aufbewahrungseinstellungen markiert haben, ist im Papierkorb.

journal.window.home.originals.copy.confirm.recover.go = sie zu Ende führen

journal.window.home.originals.copy.confirm.recover.heading = die gestoppten Löschungen fertigstellen?

journal.window.home.originals.copy.done.clause.halted = dort hat es angehalten.

journal.window.home.originals.copy.done.clause.nothing.deleted = nichts wurde gelöscht.

journal.window.home.originals.copy.done.declined.failed = es ließ sich nicht von der Liste nehmen, also bleibt es. nichts wurde gelöscht.

journal.window.home.originals.copy.done.declined.unknown = nichts wurde gelöscht. es ist unklar, ob es noch auf der Liste ist.

journal.window.home.originals.copy.done.kept.offload = vorerst behalten. es kommt nach deiner nächsten Sicherung zurück.

journal.window.home.originals.copy.done.kept.policy = vorerst behalten. es kommt wieder, wenn dieser Teil der Liste das nächste Mal neu aufgebaut wird.

journal.window.home.originals.copy.done.open.trash = Papierkorb öffnen

journal.window.home.originals.copy.done.recover.failed = sie konnten nicht fertiggestellt werden. was übrig ist, ist noch da.

journal.window.home.originals.copy.done.recover.unknown = das Zu-Ende-Führen hat aufgehört, bevor sich bestätigen ließ, was geschehen ist. es ist unklar, was abgeschlossen wurde.

journal.window.home.originals.copy.done.recovered = die Löschungen, die angehalten hatten, sind jetzt abgeschlossen.

journal.window.home.originals.copy.done.recovered.leftover = einiges ist fertig. mindestens eine Löschung, die stehen geblieben ist, wartet noch.

journal.window.home.originals.copy.done.recovered.none = es war nichts mehr fertigzustellen.

journal.window.home.originals.copy.done.unknown = das Löschen hat aufgehört, bevor sich bestätigen ließ, was geschehen ist. es ist unklar, was gelöscht wurde und was nicht.

journal.window.home.originals.copy.failed.badge = nicht abgeschlossen

journal.window.home.originals.copy.failed.body = eine Löschung, die du gestartet hast, ist auf halbem Weg stehen geblieben. sie zu Ende zu führen legt jedes Original zurück, das sie noch nicht gelöscht hatte, und der Rest bleibt gelöscht.

journal.window.home.originals.copy.failed.finish = die Löschung zu Ende führen

journal.window.home.originals.copy.operation.completed = Löschung ist fertig.

journal.window.home.originals.copy.operation.held = das Löschen ist nicht vollständig abgeschlossen.

journal.window.home.originals.copy.operation.record.unavailable = der Datensatz der Löschung ist nicht verfügbar.

journal.window.home.originals.copy.operation.refused = das Löschen hat nicht begonnen.

journal.window.home.originals.copy.operation.running = die Löschung läuft.

journal.window.home.originals.copy.operation.stop = Löschen stoppen

journal.window.home.originals.copy.operation.stop.pending = Stopp wird angefordert.

journal.window.home.originals.copy.operation.stopping = Stopp angefordert. wartet, bis die Arbeit fertig ist.

journal.window.home.originals.copy.operation.submitting = deine Löschanfrage wird gesendet.

journal.window.home.originals.copy.operation.unavailable = das aktuelle Ergebnis konnte nicht geprüft werden.

journal.window.home.originals.copy.operation.unknown = es ist unklar, was gelöscht wurde und was nicht.

journal.window.home.originals.copy.page.selection = die Auswahl gilt für diese Seite und wird aufgehoben, wenn du die Seiten wechselst.

journal.window.home.originals.copy.row.delete = Originale löschen

journal.window.home.originals.copy.row.keep = vorerst behalten

journal.window.home.originals.heading.count = { $quantity ->
        [one] Originale · 1 Element
       *[other] Originale · { $count } Elemente
    }

journal.window.home.originals.page.item.range = Elemente { $first_item }–{ $last_item } von { $total_items }

journal.window.home.originals.receipt.deleted = { $quantity ->
    [one] 1 Original gelöscht.
   *[other] { $original_count } Originale gelöscht.
    }

journal.window.home.originals.receipt.kept = { $quantity ->
    [one] 1 vorerst behalten.
   *[other] { $original_count } vorerst behalten.
    }

journal.window.home.originals.receipt.moved.to.trash = { $quantity ->
    [one] 1 Original in den Papierkorb verschoben.
   *[other] { $original_count } Originale in den Papierkorb verschoben.
    }

journal.window.home.originals.receipt.none.deleted = { $quantity ->
    [one] es konnte nicht gelöscht werden. nichts wurde gelöscht.
   *[other] sie konnten nicht gelöscht werden. nichts wurde gelöscht.
    }

journal.window.home.originals.receipt.not.kept = { $quantity ->
    [one] 1 mehr ließ sich nicht von der Liste nehmen, also bleibt es.
   *[other] { $remaining_count } mehr ließen sich nicht von der Liste nehmen, also bleiben sie.
    }

journal.window.home.originals.receipt.not.removed = { $quantity ->
    [one] 1 mehr konnte nicht gelöscht werden.
   *[other] { $remaining_count } mehr konnten nicht gelöscht werden.
    }

journal.window.home.originals.receipt.refused.named = { $original_name }: { $reason }

journal.window.home.originals.receipt.refused.reason = { $reason }

journal.window.home.originals.receipt.space.after.erase = { $quantity ->
    [one] es wird { $erase_day } endgültig gelöscht, und sein Speicherplatz kommt dann zurück. du kannst es früher aus dem Papierkorb endgültig löschen.
   *[other] sie werden { $erase_day } endgültig gelöscht, und ihr Speicherplatz kommt dann zurück. du kannst sie früher aus dem Papierkorb endgültig löschen.
    }

journal.window.home.originals.receipt.space.without.erase.day = { $quantity ->
    [one] sein Platz kommt zurück, wenn es aus dem Papierkorb endgültig gelöscht wird.
   *[other] ihr Platz kommt zurück, wenn sie aus dem Papierkorb endgültig gelöscht werden.
    }

journal.window.home.originals.review.unfinished = Originale prüfen · unabgeschlossene Löschungen brauchen eine Prüfung

journal.window.home.originals.row.contents.count = { $quantity ->
    [one] 1 Original · { $size }
   *[other] { $original_count } Originale · { $size }
    }

journal.window.home.originals.row.copied.by.backup = { $quantity ->
    [one] 1 Original durch deine Sicherung von deinem Gerät kopiert.
   *[other] { $original_count } Originale durch deine Sicherung von deinem Gerät kopiert.
    }

journal.window.home.originals.row.copied.by.backup.detail = { $quantity ->
    [one] es gibt keine Möglichkeit zu prüfen, ob es noch da ist.
   *[other] es gibt keine Möglichkeit zu prüfen, ob sie noch da sind.
    }

journal.window.home.originals.row.identity.stream = { $date_and_time } · { $stream_name }

journal.window.home.originals.row.marked.by.retention = { $quantity ->
    [one] 1 Original von deinen Aufbewahrungseinstellungen markiert.
   *[other] { $original_count } Originale von deinen Aufbewahrungseinstellungen markiert.
    }

journal.window.home.originals.row.marked.by.retention.detail = dieser Teil der Liste wird jeden Tag neu aufgebaut.

journal.window.home.originals.row.nothing.else.goes = { $quantity ->
    [one] es geht nichts anderes damit.
   *[other] es geht nichts anderes mit ihnen.
    }

journal.window.home.originals.selection.label = { $checkbox } dieses Element auswählen

journal.window.home.response.briefing.invalid = Fehlerhafte Briefing-Antwort

journal.window.home.response.pulse.failed = Puls konnte nicht geladen werden

journal.window.home.response.pulse.invalid = Fehlerhafte pulse-Antwort

journal.window.home.response.yesterday.failed = gestern konnte nicht geladen werden.

journal.window.home.section.heading.connections = Verbindungen

journal.window.home.section.heading.getting.started = erste Schritte

journal.window.home.section.heading.next = weiter

journal.window.home.section.heading.your.week = deine Woche

journal.window.home.security.device = der solstone-App auf{ $slot1 }{ $device }{ $slot2 }fehlt eine Sicherheitskorrektur. aktualisiere sie auf{ $slot3 }{ $version }{ $slot4 }oder neuer.

journal.window.home.security.journal = deinem Journal fehlt eine Sicherheitskorrektur. aktualisiere es auf{ $slot1 }{ $version }{ $slot2 }oder neuer.

journal.window.home.security.unnamed.device = der solstone-App auf einem deiner Geräte fehlt eine Sicherheitskorrektur. aktualisiere sie auf { $version } oder neuer.

journal.window.home.setup.action.choose = wählen →

journal.window.home.setup.action.finish = fertigstellen →

journal.window.home.setup.agent.choose.connector = in{ $slot1 }{ $agent }, wähle

journal.window.home.setup.agent.description = lass{ $slot1 }{ $agent }{ $slot2 }oder einen anderen Agenten dein Journal lesen.

journal.window.home.setup.phone.download = hol dir solstone auf dein Telefon unter{ $slot1 }{ $site }, dann kopple es mit deinem Journal.

journal.window.home.setup.speech.package.repair = repariere die journal-App oder installiere sie neu, dann starte dein Journal neu.

journal.window.home.text.backup.status.is.unavailable = Status der Sicherung ist nicht verfügbar.

journal.window.home.text.no.time.measured = keine Zeit gemessen

journal.window.home.text.originals.the.list.couldn.t.be = Originale · die Liste konnte nicht geladen werden.

journal.window.home.text.queued.work.status.is.unavailable = Status der Arbeit in der Warteschlange ist nicht verfügbar.

journal.window.home.text.service.status.is.unavailable = Dienststatus ist nicht verfügbar.

journal.window.home.text.the.backup.key.couldn.t.open = der Sicherungsschlüssel konnte dein Sicherungsziel nicht öffnen. prüfe die Sicherung im Zustand.

journal.window.home.time.ago.day = { $quantity ->
        [one] vor { $count } Tag
       *[other] vor { $count } Tagen
    }

journal.window.home.time.ago.hour = { $quantity ->
        [one] vor { $count } Stunde
       *[other] vor { $count } Stunden
    }

journal.window.home.time.ago.minute = { $quantity ->
        [one] vor { $count } Minute
       *[other] vor { $count } Minuten
    }

journal.window.home.time.ago.month = { $quantity ->
        [one] vor { $count } Monat
       *[other] vor { $count } Monaten
    }

journal.window.home.time.ago.second = { $quantity ->
        [one] vor { $count } Sekunde
       *[other] vor { $count } Sekunden
    }

journal.window.home.time.ago.week = { $quantity ->
        [one] vor { $count } Woche
       *[other] vor { $count } Wochen
    }

journal.window.home.today.activity.count = { $quantity ->
        [one] 1 Aktivität
       *[other] { $activity_count } Aktivitäten
    }

journal.window.home.today.activity.preview.count = { $quantity ->
        [one] { $preview_count } von { $activity_count } Aktivität
       *[other] { $preview_count } von { $activity_count } Aktivitäten
    }

journal.window.home.today.duration.hour.and.minute.count = { $quantity ->
       [one] { $hours } h { $minute_quantity ->
       [one] { $minutes } min
      *[other] { $minutes } min
    }
      *[other] { $hours } h { $minute_quantity ->
       [one] { $minutes } min
      *[other] { $minutes } min
    }
    }

journal.window.home.today.duration.hour.count = { $quantity ->
       [one] { $hours } h
      *[other] { $hours } h
    }

journal.window.home.today.duration.minute.count = { $quantity ->
       [one] { $minutes } min
      *[other] { $minutes } min
    }

journal.window.home.today.facets.measured.count = { $quantity ->
        [one] Zeit in jeder Facette · gemessen für { $measured_count } von 1 Aktivität
       *[other] Zeit in jeder Facette · gemessen für { $measured_count } von { $activity_count } Aktivitäten
    }

journal.window.home.visible.text.add.your.phone = dein Telefon hinzufügen

journal.window.home.visible.text.and.authenticate = und authentifiziere dich.

journal.window.home.visible.text.choose.another.way.when.you.re = wähle einen anderen Weg, wenn du bereit bist. dein Journal behält bis dahin alles.

journal.window.home.visible.text.connect.an.agent = Agenten verbinden

journal.window.home.visible.text.connections.are.unavailable.right.now = Verbindungen sind gerade nicht verfügbar.

journal.window.home.visible.text.connections.found.in.your.journal = in deinem Journal gefundene Verbindungen

journal.window.home.visible.text.couldn.t.be.written.out = ließ sich nicht ausschreiben.

journal.window.home.visible.text.downloading.the.local.model = das lokale Modell wird heruntergeladen

journal.window.home.visible.text.downloading.the.speech.model = das Sprachmodell wird heruntergeladen

journal.window.home.visible.text.from = von

journal.window.home.visible.text.from.your.devices.and.apps = von deinen Geräten und Apps

journal.window.home.visible.text.hasn.t.been.written.out.yet = ist noch nicht ausformuliert.

journal.window.home.visible.text.it.appears.here.as.text = es erscheint hier als Text.

journal.window.home.visible.text.leave.out.of.this.week = aus dieser Woche auslassen

journal.window.home.visible.text.left.out.of.this.week = aus dieser Woche ausgelassen ·

journal.window.home.visible.text.loading.your.week = deine Woche wird geladen…

journal.window.home.visible.text.next = weiter

journal.window.home.visible.text.no.direct.connections.yet = noch keine direkten Verbindungen.

journal.window.home.visible.text.no.named.connections.yet = noch keine benannten Verbindungen.

journal.window.home.visible.text.no.recent.activity.yet.today = heute noch keine Aktivität aus letzter Zeit.

journal.window.home.visible.text.no.speech.was.found.in.the = kein Gesprochenes gefunden im Segment von

journal.window.home.visible.text.nothing.from.yesterday.to.write.up = nichts von gestern zum Aufschreiben.

journal.window.home.visible.text.nothing.from.yesterday.was.written.up = von gestern wurde nichts aufgeschrieben.

journal.window.home.visible.text.nothing.with.sound.has.reached.your = noch hat nichts mit Ton dein Journal erreicht. schalte das Mikrofon in der solstone-App ein, um das zu versuchen.

journal.window.home.visible.text.often.around.events.only = oft in der Nähe, nur Termine

journal.window.home.visible.text.on.your.calendar.only = nur in deinem Kalender

journal.window.home.visible.text.on.your.journal.s.computer.run = auf dem Computer deines Journals, führe aus:

journal.window.home.visible.text.only.this.week.changes.the.memory = nur diese Woche ändert sich. das Gedächtnis bleibt in deinem Journal.

journal.window.home.visible.text.opens.where.it.came.from.so = öffnet, woher es kam, damit du es prüfen kannst.

journal.window.home.visible.text.previous = vorherige

journal.window.home.visible.text.say.a.sentence.out.loud = sag einen Satz laut

journal.window.home.visible.text.some.source.material.was.unavailable = einiges Quellmaterial war nicht verfügbar:

journal.window.home.visible.text.speech.from = Gesprochenes von

journal.window.home.visible.text.the.local.model.didn.t.finish = das lokale Modell wurde nicht fertig heruntergeladen.

journal.window.home.visible.text.the.segment.from = das Segment von

journal.window.home.visible.text.the.speech.model.didn.t.finish = das Modell für Gesprochenes wurde nicht fertig heruntergeladen.

journal.window.home.visible.text.then.type = dann tippe

journal.window.home.visible.text.this.isn.t.right = das ist nicht richtig

journal.window.home.visible.text.try.again = erneut versuchen

journal.window.home.visible.text.waiting.for.the.next.segment.this = wartet auf das nächste Segment. das kann ein paar Minuten dauern.

journal.window.home.visible.text.was.written.out = wurde ausgeschrieben.

journal.window.home.visible.text.what.this.summary.covers.wasn.t = was diese Zusammenfassung umfasst, wurde nicht mit ihr gespeichert.

journal.window.home.visible.text.yesterday.is.being.written.up = gestern wird gerade aufgeschrieben…

journal.window.home.visible.text.your.journal.keeps.everything.until.you = dein Journal behält alles, bis du es tust.

journal.window.home.vitals.last.check.timestamp = letzte Prüfung: { $checked_at }.

journal.window.home.vitals.last.known = zuletzt bekannt:{ $slot1 }{ $issue }

journal.window.home.vitals.last.reached.timestamp = zuletzt dein Journal erreicht { $last_reached_at }

journal.window.home.week.card.empty = nichts aus dieser Woche wird angezeigt.

journal.window.home.week.card.first.choose = deine erste Woche kommt, sobald du wählst, wo das Denken geschieht.

journal.window.home.week.card.first.due.friday = deine erste Woche kommt am Freitag.

journal.window.home.week.card.first.due.monday = deine erste Woche kommt am Montag.

journal.window.home.week.card.first.due.saturday = deine erste Woche kommt am Samstag.

journal.window.home.week.card.first.due.sunday = deine erste Woche kommt am Sonntag.

journal.window.home.week.card.first.due.thursday = deine erste Woche kommt am Donnerstag.

journal.window.home.week.card.first.due.tuesday = deine erste Woche kommt am Dienstag.

journal.window.home.week.card.first.due.wednesday = deine erste Woche kommt am Mittwoch.

journal.window.home.week.card.next.choose = deine nächste Woche kommt, sobald du wählst, wo das Denken stattfindet.

journal.window.home.week.card.next.due.friday = deine nächste Woche kommt am Freitag.

journal.window.home.week.card.next.due.monday = deine nächste Woche kommt am Montag.

journal.window.home.week.card.next.due.saturday = deine nächste Woche kommt am Samstag.

journal.window.home.week.card.next.due.sunday = deine nächste Woche kommt am Sonntag.

journal.window.home.week.card.next.due.thursday = deine nächste Woche kommt am Donnerstag.

journal.window.home.week.card.next.due.tuesday = deine nächste Woche kommt am Dienstag.

journal.window.home.week.card.next.due.wednesday = deine nächste Woche kommt am Mittwoch.

journal.window.home.week.card.unchecked = deine Woche konnte nicht geprüft werden.

journal.window.home.week.cell.accessible.left.out = { $weekday } { $day }, in deinem Journal, keine Erinnerung auf dieser Seite

journal.window.home.week.cell.accessible.memory = { $weekday } { $day }, eine Erinnerung von diesem Tag

journal.window.home.week.cell.accessible.not.ready = { $weekday } { $day }, war nicht rechtzeitig bereit

journal.window.home.week.cell.accessible.nothing.shared = { $weekday } { $day }, nichts in deinem Journal

journal.window.home.week.cell.accessible.unknown = { $weekday } { $day },{ $legacy_space }

journal.window.home.week.cell.accessible.unreadable = { $weekday } { $day }, ließ sich nicht lesen

journal.window.home.week.cell.left.out = in deinem Journal, keine Erinnerung auf dieser Seite

journal.window.home.week.cell.memory = eine Erinnerung von diesem Tag

journal.window.home.week.cell.not.ready = war nicht rechtzeitig bereit

journal.window.home.week.cell.nothing.shared = nichts in deinem Journal

journal.window.home.week.cell.unreadable = ließ sich nicht lesen

journal.window.home.week.day.long.friday = Freitag

journal.window.home.week.day.long.monday = Montag

journal.window.home.week.day.long.saturday = Samstag

journal.window.home.week.day.long.sunday = Sonntag

journal.window.home.week.day.long.thursday = Donnerstag

journal.window.home.week.day.long.tuesday = Dienstag

journal.window.home.week.day.long.wednesday = Mittwoch

journal.window.home.week.day.short.fri = Fr

journal.window.home.week.day.short.mon = Mo

journal.window.home.week.day.short.sat = Sa

journal.window.home.week.day.short.sun = So

journal.window.home.week.day.short.thu = Do

journal.window.home.week.day.short.tue = Di

journal.window.home.week.day.short.wed = Mi

journal.window.home.week.day.short.with.number = { $weekday }{ $slot1 }{ $day }

journal.window.home.week.days.conjunction = { $leading }{ $slot1 }und{ $slot2 }{ $last }

journal.window.home.week.days.from = von{ $slot1 }{ $days }.

journal.window.home.week.days.unreadable = etwas von{ $slot1 }{ $days }{ $slot2 }konnte nicht gelesen werden.

journal.window.home.week.days.unready = { $quantity ->
        [one] { $days } war nicht rechtzeitig bereit.
       *[other] { $days } waren nicht rechtzeitig bereit.
    }

journal.window.home.week.end = das ist die Woche.

journal.window.home.week.intro.empty = nichts aus dieser Woche ist auf dieser Seite.

journal.window.home.week.intro.memories = { $quantity ->
        [one] deine Woche, eine Erinnerung von einem Tag.
       *[other] deine Woche, eine Erinnerung von jedem der { $count } Tage.
    }

journal.window.home.week.leave.out.apply.failed = das konnte nicht ausgelassen werden. nichts hat sich geändert.

journal.window.home.week.leave.out.undo.failed = das konnte nicht zurückgebracht werden. nichts hat sich geändert.

journal.window.home.week.left.out.unreadable = deine ausgelassenen Erinnerungen ließen sich nicht prüfen, also wird alles angezeigt.

journal.window.home.week.load.failed = diese Woche ließ sich nicht laden. lade neu und versuch es noch einmal.

journal.window.home.week.menu.accessible.name = mehr für{ $slot1 }{ $day }

journal.window.home.week.month.april = April

journal.window.home.week.month.august = August

journal.window.home.week.month.december = Dezember

journal.window.home.week.month.february = Februar

journal.window.home.week.month.january = Januar

journal.window.home.week.month.july = Juli

journal.window.home.week.month.june = Juni

journal.window.home.week.month.march = März

journal.window.home.week.month.may = Mai

journal.window.home.week.month.november = November

journal.window.home.week.month.october = Oktober

journal.window.home.week.month.september = September

journal.window.home.week.navigation.next = { $week } →

journal.window.home.week.navigation.previous = ← { $week }

journal.window.home.week.peek.briefing = aus dem Tagesbriefing für{ $slot1 }{ $day }

journal.window.home.week.peek.moment = diesen Moment öffnen →

journal.window.home.week.peek.thinking = es in Denken öffnen →

journal.window.home.week.said.heading = von dir gesagt

journal.window.home.week.said.leave.out.scope = nur diese Woche ändert sich. was du gesagt hast, bleibt in deinem Journal.

journal.window.home.week.said.open = dort öffnen, wo du es gesagt hast →

journal.window.home.week.said.unreadable = die Liste "von dir gesagt" konnte nicht gelesen werden.

journal.window.home.week.source.link = Quelle

journal.window.home.week.title.current = Woche von{ $slot1 }{ $month }{ $slot2 }{ $day }

journal.window.home.week.title.with.year = Woche vom{ $slot1 }{ $day }.{ $slot2 }{ $month }{ $slot3 }{ $year }

journal.window.home.week.unreadable.current = die Woche von{ $slot1 }{ $month }{ $slot2 }{ $day }{ $slot3 }konnte nicht gelesen werden.

journal.window.home.week.unreadable.with.year = Woche von{ $slot1 }{ $month }{ $slot2 }{ $day },{ $slot3 }{ $year }, ließ sich nicht lesen.

journal.window.home.welcome.first.week.framing = das meiste, was dein Journal behält, wird nach etwa einer Woche nützlich, sobald dein Journal genug deiner Tage darin hat, um Muster zu zeigen. vorerst ist hier, was schon geschieht:

journal.window.home.window.explanation.analysis.will.be.available.after.the = die Analyse ist nach dem nächsten Verarbeitungszyklus verfügbar.

journal.window.home.window.explanation.backup.status.is.unavailable.right.now = der Status der Sicherung ist gerade nicht verfügbar.

journal.window.home.window.explanation.choose.where.the.thinking.happens = wähle, wo das Denken stattfindet

journal.window.home.window.explanation.couldn.t.bring.this.back.nothing = das konnte nicht zurückgebracht werden. nichts hat sich geändert.

journal.window.home.window.explanation.couldn.t.leave.this.out.nothing = das konnte nicht ausgelassen werden. nichts hat sich geändert.

journal.window.home.window.explanation.couldn.t.refresh.narrative.showing.last = Narrativ konnte nicht aktualisiert werden. letzter bekannter Stand wird gezeigt.

journal.window.home.window.explanation.couldn.t.refresh.service.status.showing = Dienststatus konnte nicht aktualisiert werden. letzter bekannter Zustand wird angezeigt.

journal.window.home.window.explanation.finish.setting.up.confidential.processing = die vertrauliche Verarbeitung fertig einrichten

journal.window.home.window.explanation.finish.setting.up.local.thinking = lokales Denken fertig einrichten

journal.window.home.window.explanation.finish.setting.up.your.own.model = dein eigenes Modell fertig einrichten

journal.window.home.window.explanation.no.narrative.yet.one.will.appear = noch keine Erzählung. eine erscheint, sobald etwas Audio in dein Journal eingeht.

journal.window.home.window.explanation.queued.work.status.is.unavailable.right = der Status der Arbeit in der Warteschlange ist gerade nicht verfügbar.

journal.window.home.window.explanation.reload.to.try.again = neu laden, um es erneut zu versuchen.

journal.window.home.window.explanation.service.status.is.unavailable.right.now = Dienststatus ist gerade nicht verfügbar.

journal.window.home.window.explanation.this.is.where.your.day.comes = hier kommt dein Tag zusammen: was geschehen ist, aufgeschrieben, und die Personen und Dinge, um die es ging. Abschnitte erscheinen hier, so wie das, was du mit der solstone-App teilst, in dein Journal eingeht.

journal.window.home.window.explanation.time.in.each.facet = Zeit in jeder Facette

journal.window.home.window.explanation.yesterday.shows.here.once.confidential.processing = gestern erscheint hier, sobald die vertrauliche Verarbeitung eingerichtet ist.

journal.window.home.window.explanation.yesterday.shows.here.once.local.thinking = gestern erscheint hier, sobald lokales Denken eingerichtet ist.

journal.window.home.window.explanation.yesterday.shows.here.once.you.choose = gestern erscheint hier, sobald du wählst, wo das Denken geschieht.

journal.window.home.window.explanation.yesterday.shows.here.once.your.own = gestern erscheint hier, sobald dein eigenes Modell eingerichtet ist.

journal.window.home.window.explanation.yesterday.shows.here.the.morning.after = gestern erscheint hier am Morgen, nachdem etwas in deinem Journal angekommen ist.

journal.window.home.window.explanation.your.first.day.shows.here.by = dein erster Tag erscheint hier spätestens um 10 Uhr, nach der ersten Nacht deines Journals.

journal.window.home.window.explanation.your.journal.s.computer.can.t = der Computer deines Journals kann lokales Denken nicht ausführen

journal.window.loading.activities = Aktivitäten werden geladen…

journal.window.loading.agents = Agenten werden geladen…

journal.window.loading.backup = Sicherung wird geladen…

journal.window.loading.day = Tag wird geladen…

journal.window.loading.entities = Entitäten werden geladen…

journal.window.loading.facets = Facetten werden geladen…

journal.window.loading.health = Zustand wird geladen…

journal.window.loading.home = dein Tag wird geladen…

journal.window.loading.import = Importe werden geladen…

journal.window.loading.map = Karte wird geladen…

journal.window.loading.more = Mehr wird geladen…

journal.window.loading.network = Geräte werden geladen…

journal.window.loading.profile = Profil wird geladen…

journal.window.loading.records = Datensätze werden geladen…

journal.window.loading.search = Suche wird geladen…

journal.window.loading.settings = Einstellungen werden geladen…

journal.window.loading.sources = Quellen werden geladen…

journal.window.loading.speakers = Sprecher werden geladen…

journal.window.loading.stats = Statistiken werden geladen…

journal.window.loading.support = Support wird geladen…

journal.window.loading.thinking = Denkeinstellungen werden geladen…

journal.window.loading.transcripts = Transkripte werden geladen…

journal.window.loading.year = Jahr wird geladen…

journal.window.more.agents.count = { $quantity ->
        [one] { $count } verbunden
       *[other] { $count } verbunden
    }

journal.window.more.coverage.since = seit{ $slot1 }{ $date }

journal.window.more.entities.count = { $quantity ->
        [one] { $count } behalten
       *[other] { $count } behalten
    }

journal.window.more.field.label.filter.views = Ansichten filtern

journal.window.more.field.placeholder.type.to.filter.views = tippen, um Ansichten zu filtern

journal.window.more.group.runs = wie es läuft

journal.window.more.group.shape = was du gestaltest

journal.window.more.imports.count = { $quantity ->
        [one] 1 Import
       *[other] { $count } Importe
    }

journal.window.more.page.heading.more = mehr

journal.window.more.pin.action.pin = { $view ->
        [agents] Agenten an die Seitenleiste anheften
        [activities] Aktivitäten an die Seitenleiste anheften
        [backup] Sicherung an die Seitenleiste anheften
        [body] Körper an die Seitenleiste anheften
        [curation] Vorschläge an die Seitenleiste anheften
        [entities] Entitäten an die Seitenleiste anheften
        [health] Zustand an die Seitenleiste anheften
        [home] Start an die Seitenleiste anheften
        [import] Import an die Seitenleiste anheften
        [map] Karte an die Seitenleiste anheften
        [more] Mehr an die Seitenleiste anheften
        [network] Netzwerk an die Seitenleiste anheften
        [search] Suche an die Seitenleiste anheften
        [settings] Einstellungen an die Seitenleiste anheften
        [sources] Quellen an die Seitenleiste anheften
        [speakers] Sprecher an die Seitenleiste anheften
        [stats] Statistik an die Seitenleiste anheften
        [support] Support an die Seitenleiste anheften
        [thinking] Denken an die Seitenleiste anheften
        [transcripts] Transkripte an die Seitenleiste anheften
        [year] Jahr an die Seitenleiste anheften
       *[other] { $view_name } an die Seitenleiste anheften
    }

journal.window.more.pin.action.unpin = { $view ->
        [agents] Agenten von der Seitenleiste losheften
        [activities] Aktivitäten von der Seitenleiste losheften
        [backup] Sicherung von der Seitenleiste losheften
        [body] Körper von der Seitenleiste losheften
        [curation] Vorschläge von der Seitenleiste losheften
        [entities] Entitäten von der Seitenleiste losheften
        [health] Zustand von der Seitenleiste losheften
        [home] Start von der Seitenleiste losheften
        [import] Import von der Seitenleiste losheften
        [map] Karte von der Seitenleiste losheften
        [more] Mehr von der Seitenleiste losheften
        [network] Netzwerk von der Seitenleiste losheften
        [search] Suche von der Seitenleiste losheften
        [settings] Einstellungen von der Seitenleiste losheften
        [sources] Quellen von der Seitenleiste losheften
        [speakers] Sprecher von der Seitenleiste losheften
        [stats] Statistiken von der Seitenleiste losheften
        [support] Support von der Seitenleiste losheften
        [thinking] Denken von der Seitenleiste losheften
        [transcripts] Transkripte von der Seitenleiste losheften
        [year] Jahr von der Seitenleiste losheften
       *[other] { $view_name } von der Seitenleiste losheften
    }

journal.window.more.review.count = { $quantity ->
        [one] { $count } zum Prüfen
       *[other] { $count } zum Prüfen
    }

journal.window.more.suggestions.pending = { $count }{ $slot1 }zum Prüfen

journal.window.more.text.no.devices.yet = noch keine Geräte

journal.window.more.text.none.yet = noch nichts

journal.window.more.text.not.set.up.yet = noch nicht eingerichtet

journal.window.more.text.nothing.to.review = nichts zu prüfen

journal.window.more.text.nothing.yet = noch nichts

journal.window.more.tooltip.always.in.the.side.rail = immer in der Seitenleiste

journal.window.more.visible.text.couldn.t.save.that.try.again = das konnte nicht gespeichert werden. versuch es noch einmal.

journal.window.more.visible.text.every.view.your.journal.has.what = jede Ansicht, die dein Journal hat: was jede zeigt, und woher ihre Inhalte kommen.

journal.window.more.visible.text.no.views.match.that = keine Ansichten passen dazu.

journal.window.more.window.explanation.fix.and.tidy.what.it.keeps = ausbessern und aufräumen, was es behält

journal.window.more.window.explanation.set.it.up.and.keep.it = richte es ein und lass es laufen

journal.window.more.window.explanation.what.your.journal.keeps = was dein Journal behält

journal.window.more.window.explanation.your.days.as.your.journal.holds = deine Tage, wie dein Journal sie hält

journal.window.search.agent.action = Aktion

journal.window.search.agent.activity = Aktivität

journal.window.search.agent.audio = Audio

journal.window.search.agent.audio.transcript = Audio-Transkript

journal.window.search.agent.body = Körper

journal.window.search.agent.brief = Kurzfassung

journal.window.search.agent.browser = Browser

journal.window.search.agent.calendar = Kalender

journal.window.search.agent.conversation.transcript = Gesprächstranskript

journal.window.search.agent.describe = beschreiben

journal.window.search.agent.desktop.activity = Schreibtischaktivität

journal.window.search.agent.document.transcript = Dokument-Transkript

journal.window.search.agent.documents = Dokumente

journal.window.search.agent.entity.attached = Entität:angehängt

journal.window.search.agent.entity.detected = entity:detected

journal.window.search.agent.event = Ereignis

journal.window.search.agent.event.transcript = Ereignis-Transkript

journal.window.search.agent.flow = Flow

journal.window.search.agent.granola = Granola

journal.window.search.agent.image.transcript = Bild-Transkript

journal.window.search.agent.import = Import

journal.window.search.agent.imported = importiert

journal.window.search.agent.location = Ort

journal.window.search.agent.memory = Erinnerung

journal.window.search.agent.morning.briefing = Morgenbriefing

journal.window.search.agent.note.transcript = Notiz-Transkript

journal.window.search.agent.objects = Objekte

journal.window.search.agent.observation = Beobachtung

journal.window.search.agent.possible.names = mögliche Namen

journal.window.search.agent.protected.video = geschütztes Video

journal.window.search.agent.pulse = Puls

journal.window.search.agent.reflection = Rückblick

journal.window.search.agent.screen = Bildschirm

journal.window.search.agent.screen.text = Bildschirmtext

journal.window.search.agent.screenshot.transcript = Bildschirmfoto-Transkript

journal.window.search.agent.sense = Sinn

journal.window.search.agent.session.review = Sitzungsprüfung

journal.window.search.agent.story = Geschichte

journal.window.search.agent.topics = Themen

journal.window.search.count.results.across.days = { $match_quantity ->
    [one] { $day_quantity ->
        [one] { $match_count } Treffer über { $day_count } Tag.
       *[other] { $match_count } Treffer über { $day_count } Tage.
    }
   *[other] { $day_quantity ->
        [one] { $match_count } Treffer über { $day_count } Tag.
       *[other] { $match_count } Treffer über { $day_count } Tage.
    }
}

journal.window.search.count.results.shown.for.day = { $match_quantity ->
    [one] { $shown_quantity ->
        [one] zeigt die obersten { $shown_count } von { $match_count } Treffer für diesen Tag.
       *[other] zeigt die obersten { $shown_count } von { $match_count } Treffer für diesen Tag.
    }
   *[other] { $shown_quantity ->
        [one] zeigt die obersten { $shown_count } von { $match_count } Treffern für diesen Tag.
       *[other] zeigt die obersten { $shown_count } von { $match_count } Treffern für diesen Tag.
    }
}

journal.window.search.date.from = von

journal.window.search.date.invalid = wähle ein Enddatum am oder nach dem Startdatum.

journal.window.search.date.range = Datumsbereich

journal.window.search.date.through = bis

journal.window.search.entry.as.indexed = wie zuletzt indexiert

journal.window.search.entry.close = Eintrag schließen

journal.window.search.entry.loading = Eintrag wird geladen…

journal.window.search.entry.open = passenden Eintrag lesen

journal.window.search.entry.reindexed = diese Datei wurde nach deiner Suche erneut indexiert, deshalb passt das vielleicht nicht zum Ergebnis

journal.window.search.entry.stored.record = der gespeicherte Datensatz

journal.window.search.heading = dein Journal durchsuchen

journal.window.search.hit.names = Namen

journal.window.search.intro = finde Wörter und Ausdrücke in deinem Journal, dann lies den passenden Eintrag.

journal.window.search.link.agents = in Agenten öffnen

journal.window.search.link.day = diesen Tag öffnen →

journal.window.search.link.entity = Entität öffnen

journal.window.search.link.map = auf der Karte öffnen

journal.window.search.link.transcript = Transkript öffnen

journal.window.search.link.week = jene Woche öffnen →

journal.window.search.message.assistant = Assistent

journal.window.search.message.everyone = alle

journal.window.search.message.meta = { $sender } an { $recipients }

journal.window.search.message.unknown = Unbekannt

journal.window.search.message.you = du

journal.window.search.paging.accessible.name = Suchergebnisseiten

journal.window.search.paging.day = mehr von diesem Tag

journal.window.search.paging.first = zurück zu den ersten Ergebnissen

journal.window.search.paging.newer = neuere passende Tage

journal.window.search.paging.older = ältere passende Tage

journal.window.search.query.label = Wörter oder Ausdruck

journal.window.search.query.placeholder = was möchtest du finden?

journal.window.search.query.submit = suchen

journal.window.search.response.agent.name = Agent muss ein bloßer Talentname sein

journal.window.search.response.date.format = { $field }{ $slot1 }muss { $slot2 } sein

journal.window.search.response.date.real = { $field }{ $slot1 }muss ein echter Tag sein

journal.window.search.response.date.reverse = { $slot1 } muss <= { $slot2 } sein

journal.window.search.response.day.format = { $slot1 } muss { $slot2 } sein

journal.window.search.response.entry.absent = dieser Eintrag ist nicht mehr indexiert. suche erneut, um die Ergebnisse zu aktualisieren.

journal.window.search.response.entry.invalid = ungültige Eintragsreferenz

journal.window.search.response.entry.large = dieser Eintrag ist zu lang, um ihn hier anzuzeigen.

journal.window.search.response.file.not.found = diese Datei ist nicht verfügbar.

journal.window.search.response.file.read.failed = diese Datei konnte nicht gelesen werden.

journal.window.search.response.filter.agent = es gibt keinen Agenten namens{ $slot1 }{ $name }. Agenten:{ $slot2 }{ $available }

journal.window.search.response.filter.facet = es gibt keine Facette namens{ $slot1 }{ $name }. Facetten:{ $slot2 }{ $available }

journal.window.search.response.index.read.detail = der Suchindex konnte nicht gelesen werden. versuch es noch einmal.

journal.window.search.response.invalid.day = dieser Tag konnte nicht verwendet werden.

journal.window.search.response.invalid.path = dieser Pfad konnte nicht verwendet werden.

journal.window.search.response.invalid.value = einer dieser Werte konnte nicht verwendet werden.

journal.window.search.response.read.agent.absent = keine Ausgabe dieser Art wurde für diesen Tag gefunden

journal.window.search.response.read.bound = { $slot1 } muss { $slot2 } sein für { $slot3 }-Lesezugriffe

journal.window.search.response.read.entity = Entitätsergebnisse haben keine Datei zum Lesen

journal.window.search.response.read.entry.fields = ein indizierter Eintrag braucht { $slot1 }, { $slot2 } und { $slot3 } aus demselben Suchergebnis

journal.window.search.response.read.failed = Journal-Datei konnte nicht gelesen werden.

journal.window.search.response.read.file.absent = Journaldatei nicht gefunden

journal.window.search.response.read.mixed.entry = ein indexierter Eintrag kann nicht mit { $slot1 }, { $slot2 } oder { $slot3 } kombiniert werden

journal.window.search.response.read.mixed.file = { $slot1 } lässt sich nicht mit { $slot2 }, { $slot3 } oder { $slot4 } kombinieren

journal.window.search.response.read.required = { $slot1 } oder { $slot2 } ist erforderlich

journal.window.search.response.read.suffix = entferne das Suchergebnis-Suffix { $slot1 }, bevor du einen Pfad liest

journal.window.search.response.search.failed = dein Journal konnte gerade nicht durchsucht werden.

journal.window.search.response.segment.invalid = ungültiger Segmentschlüssel

journal.window.search.status.details = Details

journal.window.search.status.entry.failed = dieser Eintrag konnte nicht geöffnet werden.{ $slot1 }{ $detail }

journal.window.search.status.entry.failure.details = dieser Eintrag ließ sich nicht öffnen. { $detail } { $diagnostic }

journal.window.search.status.entry.failure.retry = dieser Eintrag ließ sich nicht öffnen. versuch es noch einmal.

journal.window.search.status.entry.failure.retry.diagnostic = dieser Eintrag konnte nicht geöffnet werden. versuch es noch einmal. { $diagnostic }

journal.window.search.status.failure.index.absent = dein Journal ließ sich nicht durchsuchen: dein Journal hat noch keinen Suchindex.

journal.window.search.status.failure.index.empty = dein Journal konnte nicht durchsucht werden: es ist noch nichts indexiert.

journal.window.search.status.failure.index.locked = dein Journal ließ sich nicht durchsuchen: der Suchindex ist gerade belegt.

journal.window.search.status.failure.index.unreadable = dein Journal konnte nicht durchsucht werden: der Suchindex konnte nicht gelesen werden.

journal.window.search.status.failure.retry = dein Journal ließ sich nicht durchsuchen. versuch es noch einmal.

journal.window.search.status.index.rebuilding = die Suche wird neu aufgebaut, daher können ältere Tage vorerst fehlen.

journal.window.search.status.initial = durchsuch dein ganzes Journal. setz eine "genaue Phrase" in Anführungszeichen, oder grenz oben auf einen Tagesbereich ein, für schnellere Ergebnisse.

journal.window.search.status.loading = dein Journal wird durchsucht…

journal.window.search.status.narrow = füge Wörter hinzu, um diese Treffer einzugrenzen.

journal.window.search.status.none = keine Treffer. versuch weniger Wörter oder einen breiteren Datumsbereich.

journal.window.search.status.progress = sucht noch…{ $slot1 }{ $seconds }{ $slot2 }s.

journal.window.search.status.progress.all.days = sucht noch…{ $slot1 }{ $seconds }{ $slot2 }s. ein einzelner Tag ist meist viel schneller.

journal.window.search.status.relaxed = nichts hat deiner genauen Suche entsprochen, also sind das breitere Treffer.

journal.window.search.status.retry = erneut versuchen

journal.window.search.status.search.failed.other = dein Journal konnte nicht durchsucht werden.{ $slot1 }{ $detail }

journal.window.shared.api.authentication.required = Authentifizierung erforderlich

journal.window.shared.api.malformed.response = Fehlerhafte Serverantwort

journal.window.shared.api.request.failed = Anfrage fehlgeschlagen

journal.window.shared.api.request.failed.http = Anfrage ist nicht gelungen (HTTP{ $slot1 }{ $http_status })

journal.window.shared.api.window.explanation.couldn.t.finish.that.request = diese Anfrage konnte nicht abgeschlossen werden.

journal.window.shared.app.disclosure.label.show.details = Details anzeigen

journal.window.shared.app.label.try.now = Jetzt versuchen

journal.window.shared.app.notice.background.task = { $app_name }{ $slot1 }Hintergrundaufgabe

journal.window.shared.app.notification.segments = { $count }{ $slot1 }Segmente

journal.window.shared.app.notification.title = Benachrichtigung

journal.window.shared.app.request.failed = Anfrage fehlgeschlagen

journal.window.shared.app.visible.text.click.to.copy = (zum Kopieren klicken)

journal.window.shared.app.visible.text.copied = (kopiert)

journal.window.shared.app.visible.text.reason.code = Ursachencode:

journal.window.shared.app.visible.text.reference = Referenz:

journal.window.shared.app.visible.text.server.reason = Servergrund:

journal.window.shared.app.visible.text.still.working = arbeitet noch…

journal.window.shared.app.visible.text.time = Zeit:

journal.window.shared.app.window.explanation.also.remove.it.from.your.backups = entferne es auch aus deinen Sicherungen

journal.window.shared.app.window.explanation.also.remove.them.from.your.backups = sie auch aus deinen Sicherungen entfernen

journal.window.shared.app.window.explanation.backup.is.paused.so.this.happens = die Sicherung ist pausiert, also passiert das, sobald du sie fortsetzt.

journal.window.shared.app.window.explanation.couldn.t.load.this.section = dieser Abschnitt konnte nicht geladen werden

journal.window.shared.app.window.explanation.couldn.t.reach.your.journal.some = dein Journal war nicht erreichbar. einige Hintergrundaktualisierungen sind pausiert.

journal.window.shared.app.window.explanation.reload.to.try.again = neu laden, um es erneut zu versuchen.

journal.window.shared.app.window.explanation.removing.it.from.your.backups.next = es wird als Nächstes aus deinen Sicherungen entfernt.

journal.window.shared.app.window.explanation.removing.it.from.your.backups.once = es aus deinen Sicherungen entfernen, sobald du die Sicherung fortsetzt.

journal.window.shared.app.window.explanation.removing.them.from.your.backups.next = sie als Nächstes aus deinen Sicherungen entfernen.

journal.window.shared.app.window.explanation.removing.them.from.your.backups.once = sie aus deinen Sicherungen entfernen, sobald du die Sicherung fortsetzt.

journal.window.shared.app.window.explanation.then.restoring.your.journal.from.a = dann kann das Wiederherstellen deines Journals aus einer Sicherung es nicht zurückholen. deine Sicherungen bleiben, mit allem anderen darin.

journal.window.shared.app.window.explanation.then.restoring.your.journal.from.a.variant2 = dann kann das Wiederherstellen deines Journals aus einer Sicherung sie nicht zurückbringen. deine Sicherungen bleiben, mit allem anderen darin.

journal.window.shared.app.window.explanation.your.journal.couldn.t.check.your = dein Journal konnte deine Sicherungen gerade eben nicht prüfen, daher lässt dies sie, wie sie sind.

journal.window.shared.console.link.message.count = { $quantity ->
        [one] Systemnachrichten ({ $count })
       *[other] Systemnachrichten ({ $count })
    }

journal.window.shared.console.link.unread.count = { $quantity ->
        [one] Systemnachrichten, { $count } ungelesen
       *[other] Systemnachrichten, { $count } ungelesen
    }

journal.window.shared.console.snapshot.message.count = { $quantity ->
        [one] Abbild von { $count } Systemnachricht
       *[other] Abbild von { $count } Systemnachrichten
    }

journal.window.shared.date.format.duration.hour.and.minute.count = { $quantity ->
        [one] { $hours } h { $minutes_quantity ->
        [one] { $minutes } min
       *[other] { $minutes } min
    }
       *[other] { $hours } h { $minutes_quantity ->
        [one] { $minutes } min
       *[other] { $minutes } min
    }
    }

journal.window.shared.date.format.duration.hour.count = { $quantity ->
        [one] { $hours } h
       *[other] { $hours } h
    }

journal.window.shared.date.format.duration.minute.and.second.count = { $quantity ->
        [one] { $minutes } min { $seconds_quantity ->
        [one] { $seconds } s
       *[other] { $seconds } s
    }
       *[other] { $minutes } min { $seconds_quantity ->
        [one] { $seconds } s
       *[other] { $seconds } s
    }
    }

journal.window.shared.date.format.duration.minute.count = { $quantity ->
        [one] { $minutes } min
       *[other] { $minutes } min
    }

journal.window.shared.date.format.duration.second.count = { $quantity ->
        [one] { $seconds } s
       *[other] { $seconds } s
    }

journal.window.shared.date.format.duration.unavailable = Dauer nicht verfügbar

journal.window.shared.date.format.import.audio = Audio

journal.window.shared.date.format.import.document = Dokumente

journal.window.shared.date.format.import.ics = Kalender

journal.window.shared.date.format.import.image = Bilder

journal.window.shared.date.format.import.journal.archive = ein anderes Journal

journal.window.shared.date.format.import.obsidian = Notizen

journal.window.shared.date.format.imported.source.label = { $source_name }{ $slot1 }· importiert

journal.window.shared.date.format.lane.byo = dein eigenes Modell

journal.window.shared.date.format.lane.default = Verarbeitung

journal.window.shared.date.format.lane.local = lokales Denken

journal.window.shared.date.format.lane.spp = vertrauliche Verarbeitung

journal.window.shared.date.format.months.since.apr = Apr

journal.window.shared.date.format.months.since.aug = Aug

journal.window.shared.date.format.months.since.dec = Dez

journal.window.shared.date.format.months.since.feb = Feb

journal.window.shared.date.format.months.since.jan = Jan

journal.window.shared.date.format.months.since.jul = Jul

journal.window.shared.date.format.months.since.jun = Jun

journal.window.shared.date.format.months.since.mar = Mär

journal.window.shared.date.format.months.since.may = Mai

journal.window.shared.date.format.months.since.nov = Nov

journal.window.shared.date.format.months.since.oct = Okt

journal.window.shared.date.format.months.since.sep = Sep

journal.window.shared.date.format.source.browser = dein Browser

journal.window.shared.date.format.source.phone = dein Telefon

journal.window.shared.date.format.source.terminal = dein Terminal

journal.window.shared.date.format.source.unknown = Quelle unbekannt

journal.window.shared.date.format.source.watch = deine Uhr

journal.window.shared.date.format.time.unavailable = Zeit nicht verfügbar

journal.window.shared.date.nav.accessible.name.change.date.level = Datumsebene ändern

journal.window.shared.date.nav.accessible.name.go.to.today = zu heute gehen

journal.window.shared.date.nav.accessible.name.next = weiter

journal.window.shared.date.nav.accessible.name.next.day = nächster Tag

journal.window.shared.date.nav.accessible.name.previous = vorherige

journal.window.shared.date.nav.accessible.name.previous.day = vorheriger Tag

journal.window.shared.date.nav.cell.date.and.count = { $day_label }, { $quantity_label }

journal.window.shared.date.nav.days.counting = Tage werden noch gezählt…

journal.window.shared.date.nav.days.loading = Tage werden geladen…

journal.window.shared.date.nav.index.failed.day.count = { $quantity ->
        [one] 1 Tag konnte nicht gezählt werden. es wird noch versucht.
       *[other] { $count } Tage konnten nicht gezählt werden. es wird noch versucht.
    }

journal.window.shared.date.nav.months.label.april = April

journal.window.shared.date.nav.months.label.august = August

journal.window.shared.date.nav.months.label.december = Dezember

journal.window.shared.date.nav.months.label.february = Februar

journal.window.shared.date.nav.months.label.january = Januar

journal.window.shared.date.nav.months.label.july = Juli

journal.window.shared.date.nav.months.label.june = Juni

journal.window.shared.date.nav.months.label.march = März

journal.window.shared.date.nav.months.label.may = Mai

journal.window.shared.date.nav.months.label.november = November

journal.window.shared.date.nav.months.label.october = Oktober

journal.window.shared.date.nav.months.label.september = September

journal.window.shared.date.nav.months.loading = Monate werden geladen…

journal.window.shared.date.nav.visible.text.pick.a.day = wähl einen Tag

journal.window.shared.date.nav.visible.text.today = heute

journal.window.shared.date.nav.visible.text.try.again = erneut versuchen

journal.window.shared.date.nav.week.current = Diese Woche

journal.window.shared.date.nav.week.headline.month.day = Woche von{ $slot1 }{ $month }{ $slot2 }{ $day }

journal.window.shared.date.nav.week.option.month.day = Woche von{ $slot1 }{ $month }{ $slot2 }{ $day }

journal.window.shared.date.nav.week.previous = Letzte Woche

journal.window.shared.date.nav.window.explanation.couldn.t.load.which.days.have = es konnte nicht geladen werden, welche Tage Material haben.

journal.window.shared.date.nav.window.explanation.couldn.t.load.which.months.have = es konnte nicht geladen werden, welche Monate Material haben.

journal.window.shared.day.grid.calendar.year = { $year }{ $slot1 }Kalender

journal.window.shared.day.grid.cell.date.and.count = { $day_label }: { $quantity_label }

journal.window.shared.day.grid.cell.date.and.pending.count = { $day_label }: { $quantity_label }, Zusammenfassung ausstehend

journal.window.shared.day.grid.range.anchor = beendet den Bereich von{ $slot1 }{ $start_day }

journal.window.shared.day.grid.range.cleared = Bereich aufgehoben

journal.window.shared.day.grid.range.start = Bereich beginnt{ $slot1 }{ $start_day }. wähle den letzten Tag.

journal.window.shared.day.grid.visible.text.less = weniger

journal.window.shared.day.grid.visible.text.more = mehr

journal.window.shared.day.grid.visible.text.none = nichts

journal.window.shared.day.grid.visible.text.open = öffnen →

journal.window.shared.day.grid.visible.text.pending.rollup = ausstehende Zusammenfassung

journal.window.shared.day.grid.visible.text.yes = ja

journal.window.shared.diagnostic.console.reason.code = Ursachencode

journal.window.shared.diagnostic.console.server.reason = Server-Grund

journal.window.shared.diagnostic.console.visible.text.http = · HTTP

journal.window.shared.error.handler.window.explanation.couldn.t.load.part.of.this = ein Teil dieser Seite konnte nicht geladen werden.

journal.window.shared.error.handler.window.explanation.navigation.aborted.a.read = Navigation hat einen Lesevorgang abgebrochen:

journal.window.shared.error.handler.window.explanation.something.on.this.page.didn.t = etwas auf dieser Seite hat nicht funktioniert.

journal.window.shared.gate.drawer.audio.recordings.count = { $quantity ->
    [one] Aufnahmen mit Audio: { $count }
   *[other] Aufnahmen mit Audio: { $count }
}

journal.window.shared.gate.drawer.manual.tags.count = { $quantity ->
    [one] Äußerungen, die du beigebracht hast: { $count }
   *[other] Äußerungen, die du beigebracht hast: { $count }
}

journal.window.shared.gate.drawer.median.length = Medianlänge

journal.window.shared.gate.drawer.open.label = warum noch nicht?

journal.window.shared.gate.drawer.statement.median.required.seconds = mediane Äußerungslänge { $observed } s, braucht { $minimum } s

journal.window.shared.gate.drawer.statement.median.seconds = mediane Äußerungslänge { $observed } s

journal.window.shared.gate.drawer.statement.median.unavailable = mediane Äußerungslänge k. A.

journal.window.shared.gate.drawer.statement.median.unavailable.required.seconds = mediane Äußerungslänge k. A., braucht { $minimum } s

journal.window.shared.gate.drawer.statements.observed.and.required.count = { $quantity ->
        [one] { $observed } längere Äußerung bisher, braucht { $minimum_quantity ->
        [one] { $minimum }
       *[other] { $minimum }
    }
       *[other] { $observed } längere Äußerungen bisher, braucht { $minimum_quantity ->
        [one] { $minimum }
       *[other] { $minimum }
    }
    }

journal.window.shared.gate.drawer.statements.observed.count = { $quantity ->
        [one] { $observed } längere Äußerung bisher
       *[other] { $observed } längere Äußerungen bisher
    }

journal.window.shared.gate.drawer.threshold = braucht { $threshold }

journal.window.shared.gate.drawer.window.explanation.pick.longer.statements.when.you.teach = wähle längere Äußerungen, wenn du deine Stimme anlernst.

journal.window.shared.gate.drawer.window.explanation.pick.statements.where.only.you.are = wähle Äußerungen, in denen nur du sprichst.

journal.window.shared.gate.drawer.window.explanation.teach.a.few.more.of.your = bring noch ein paar deiner längeren Äußerungen bei.

journal.window.shared.gate.drawer.window.explanation.voice.pattern.is.still.too.spread = Stimm-Muster ist noch zu weit gestreut

journal.window.shared.gate.drawer.window.explanation.you.ve.taught.enough.to.save = du hast genug beigebracht, um deine Stimme zu speichern.

journal.window.shared.import.item.dated.label = { $label } · { $date }

journal.window.shared.import.item.kind.conversation = Gespräch

journal.window.shared.import.item.kind.document = Dokument

journal.window.shared.import.item.kind.messages = Nachrichten

journal.window.shared.import.item.kind.note = Notiz

journal.window.shared.import.item.kind.photo = Foto

journal.window.shared.import.item.kind.screenshot = Bildschirmfoto

journal.window.shared.import.item.kind.transcript = Transkript

journal.window.shared.import.item.label = { $kind } · { $source }

journal.window.shared.import.item.source.chatgpt = ChatGPT

journal.window.shared.import.item.source.claude = Claude

journal.window.shared.import.item.source.evernote = Evernote

journal.window.shared.import.item.source.gemini = Gemini

journal.window.shared.import.item.source.imported = importiert

journal.window.shared.import.item.source.obsidian = Obsidian

journal.window.shared.import.item.source.pdf = PDF

journal.window.shared.import.item.text.unavailable = kein Text verfügbar

journal.window.shared.mount.workspace.button.label.try.again = erneut versuchen

journal.window.shared.mount.workspace.section.heading.couldn.t.load.this.section = dieser Abschnitt konnte nicht geladen werden

journal.window.shared.mount.workspace.visible.text.reload.to.try.again = neu laden, um es erneut zu versuchen.

journal.window.shared.question.visible.text.cancel = abbrechen

journal.window.shared.question.visible.text.name = Name

journal.window.shared.report.error.journal.version = journal{ $slot1 }{ $version }

journal.window.shared.report.error.status.text.your.versions = deine Versionen

journal.window.shared.report.error.window.explanation.nothing.leaves.this.journal.until.you = nichts verlässt dieses Journal, bis du das Formular auf der Support-Website sendest.

journal.window.shared.shell.boot.accessible.name.about = über

journal.window.shared.shell.boot.accessible.name.connecting = wird verbunden

journal.window.shared.shell.boot.button.label.try.again = erneut versuchen

journal.window.shared.shell.boot.section.heading.couldn.t.load.this.section = dieser Abschnitt konnte nicht geladen werden

journal.window.shared.shell.boot.tooltip.connecting = wird verbunden

journal.window.shared.shell.boot.visible.text.more = mehr

journal.window.shared.shell.boot.visible.text.reload.to.try.again = neu laden, um es erneut zu versuchen.

journal.window.shared.shell.boot.window.explanation.no.system.messages.this.session = keine Systemnachrichten in dieser Sitzung.

journal.window.shared.shell.boot.window.explanation.sending.reports.is.off = das Senden von Meldungen ist aus.

journal.window.shared.status.pane.accessible.label.browser.notifications.enabled = Browser-Benachrichtigungen aktiviert

journal.window.shared.status.pane.accessible.label.enable.browser.notifications = Browser-Benachrichtigungen aktivieren

journal.window.shared.status.pane.accessible.label.notifications.blocked.update.in.browser.settings = Benachrichtigungen blockiert. in den Browsereinstellungen aktualisieren

journal.window.shared.status.pane.age.elapsed.day = { $quantity ->
        [one] vor { $count } Tag
       *[other] vor { $count } Tagen
    }

journal.window.shared.status.pane.age.elapsed.hour = { $quantity ->
        [one] vor { $count } Stunde
       *[other] vor { $count } Stunden
    }

journal.window.shared.status.pane.age.elapsed.minute = { $quantity ->
        [one] vor { $count } Minute
       *[other] vor { $count } Minuten
    }

journal.window.shared.status.pane.age.elapsed.month = { $quantity ->
        [one] vor { $count } Monat
       *[other] vor { $count } Monaten
    }

journal.window.shared.status.pane.age.elapsed.second = { $quantity ->
        [one] vor { $count } Sekunde
       *[other] vor { $count } Sekunden
    }

journal.window.shared.status.pane.age.elapsed.week = { $quantity ->
        [one] vor { $count } Woche
       *[other] vor { $count } Wochen
    }

journal.window.shared.status.pane.automatic.decline = nein, danke

journal.window.shared.status.pane.automatic.enable = einschalten

journal.window.shared.status.pane.automatic.held = Aktualisierungen werden zurückgehalten

journal.window.shared.status.pane.automatic.installs = wird automatisch installiert

journal.window.shared.status.pane.automatic.paused = automatische Aktualisierungen stoppten nach zwei Versuchen, die nicht funktioniert haben.

journal.window.shared.status.pane.automatic.retry = erneut versuchen

journal.window.shared.status.pane.automatic.updates = aktualisiert automatisch

journal.window.shared.status.pane.capture.device.attention = { $device } braucht Aufmerksamkeit

journal.window.shared.status.pane.capture.last.reached.day = was es hinzufügt, hat dein Journal seit { $since_day } nicht erreicht.

journal.window.shared.status.pane.capture.more.need.attention = { $quantity ->
        [one] und { $count } mehr braucht Aufmerksamkeit
       *[other] und { $count } mehr brauchen Aufmerksamkeit
    }

journal.window.shared.status.pane.capture.rejected.upload.count = { $quantity ->
        [one] 1 Upload abgewiesen.
       *[other] { $count } Uploads abgewiesen.
    }

journal.window.shared.status.pane.connected.elapsed.day = { $quantity ->
        [one] verbunden seit { $count } Tag
       *[other] verbunden seit { $count } Tagen
    }

journal.window.shared.status.pane.connected.elapsed.hour = { $quantity ->
        [one] seit { $count } Stunde verbunden
       *[other] seit { $count } Stunden verbunden
    }

journal.window.shared.status.pane.connected.elapsed.minute = { $quantity ->
        [one] seit { $count } Minute verbunden
       *[other] seit { $count } Minuten verbunden
    }

journal.window.shared.status.pane.connected.elapsed.month = { $quantity ->
        [one] seit { $count } Monat verbunden
       *[other] seit { $count } Monaten verbunden
    }

journal.window.shared.status.pane.connected.elapsed.second = { $quantity ->
        [one] seit { $count } Sekunde verbunden
       *[other] seit { $count } Sekunden verbunden
    }

journal.window.shared.status.pane.connected.elapsed.week = { $quantity ->
        [one] seit { $count } Woche verbunden
       *[other] seit { $count } Wochen verbunden
    }

journal.window.shared.status.pane.connection.just.now = gerade eben verbunden

journal.window.shared.status.pane.rejected.elapsed.day = { $quantity ->
        [one] zuletzt vor { $count } Tag abgelehnt
       *[other] zuletzt vor { $count } Tagen abgelehnt
    }

journal.window.shared.status.pane.rejected.elapsed.hour = { $quantity ->
        [one] zuletzt vor { $count } Stunde abgelehnt
       *[other] zuletzt vor { $count } Stunden abgelehnt
    }

journal.window.shared.status.pane.rejected.elapsed.minute = { $quantity ->
        [one] zuletzt vor { $count } Minute abgelehnt
       *[other] zuletzt vor { $count } Minuten abgelehnt
    }

journal.window.shared.status.pane.rejected.elapsed.month = { $quantity ->
        [one] zuletzt vor { $count } Monat abgelehnt
       *[other] zuletzt vor { $count } Monaten abgelehnt
    }

journal.window.shared.status.pane.rejected.elapsed.second = { $quantity ->
        [one] zuletzt vor { $count } Sekunde abgelehnt
       *[other] zuletzt vor { $count } Sekunden abgelehnt
    }

journal.window.shared.status.pane.rejected.elapsed.week = { $quantity ->
        [one] zuletzt vor { $count } Woche abgelehnt
       *[other] zuletzt vor { $count } Wochen abgelehnt
    }

journal.window.shared.status.pane.security.device.update.version = bei der solstone-App auf { $device_name } fehlt eine Sicherheitskorrektur (aktualisieren auf { $fixed_version }).

journal.window.shared.status.pane.security.journal.update.version = deinem Journal fehlt eine Sicherheitskorrektur (aktualisiere auf { $fixed_version }).

journal.window.shared.status.pane.security.restart = starte dein Journal neu

journal.window.shared.status.pane.security.unnamed.device.update.version = der solstone-App auf einem deiner Geräte fehlt eine Sicherheitskorrektur (aktualisieren auf { $fixed_version }).

journal.window.shared.status.pane.status.text.a.device.isn.t.reaching.your = ein Gerät erreicht dein Journal nicht.

journal.window.shared.status.pane.status.text.a.device.needs.attention = ein Gerät braucht Aufmerksamkeit

journal.window.shared.status.pane.status.text.update.or.restart.it.on.that = aktualisiere oder starte es auf jenem Gerät neu, dann hebt ein gültiger Upload das auf.

journal.window.shared.status.pane.tooltip.browser.notifications.enabled = Browser-Benachrichtigungen aktiviert

journal.window.shared.status.pane.tooltip.enable.browser.notifications = Browser-Benachrichtigungen aktivieren

journal.window.shared.status.pane.tooltip.notifications.blocked.update.in.browser.settings = Benachrichtigungen blockiert. in den Browsereinstellungen aktualisieren

journal.window.shared.status.pane.update.just.now = gerade eben aktualisiert

journal.window.shared.status.pane.updated.elapsed.day = { $quantity ->
        [one] vor { $count } Tag aktualisiert
       *[other] vor { $count } Tagen aktualisiert
    }

journal.window.shared.status.pane.updated.elapsed.hour = { $quantity ->
        [one] vor { $count } Stunde aktualisiert
       *[other] vor { $count } Stunden aktualisiert
    }

journal.window.shared.status.pane.updated.elapsed.minute = { $quantity ->
        [one] vor { $count } Minute aktualisiert
       *[other] vor { $count } Minuten aktualisiert
    }

journal.window.shared.status.pane.updated.elapsed.month = { $quantity ->
        [one] vor { $count } Monat aktualisiert
       *[other] vor { $count } Monaten aktualisiert
    }

journal.window.shared.status.pane.updated.elapsed.second = { $quantity ->
        [one] vor { $count } Sekunde aktualisiert
       *[other] vor { $count } Sekunden aktualisiert
    }

journal.window.shared.status.pane.updated.elapsed.week = { $quantity ->
        [one] vor { $count } Woche aktualisiert
       *[other] vor { $count } Wochen aktualisiert
    }

journal.window.shared.status.pane.version.current.up.to.date = v{ $current_version } · auf dem neuesten Stand

journal.window.shared.status.pane.version.package.repository.link = aus einem Paket installiert. um Aktualisierungen über das Aktualisierungsprogramm deines Systems zu bekommen, füge das solstone-Repository hinzu: { $repository_link }

journal.window.shared.status.pane.version.unknown.up.to.date = vunbekannt · auf dem neuesten Stand

journal.window.shared.status.pane.version.update.available = v{ $current_version } · { $notice_start }Aktualisierung verfügbar (v{ $available_version }){ $notice_end }

journal.window.shared.status.pane.version.update.installed = v{ $current_version } · v{ $installed_version } ist installiert und startet, wenn dein Journal das nächste Mal startet

journal.window.shared.status.pane.visible.text.a.security.update.is.installed.restart = eine Sicherheitsaktualisierung ist installiert. starte dein Journal neu, um sie abzuschließen.

journal.window.shared.status.pane.visible.text.couldn.t.check.for.security.updates = die Suche nach Sicherheitsaktualisierungen ist nicht gelungen.

journal.window.shared.status.pane.visible.text.device.status.is.unavailable.right.now = Gerätestatus ist gerade nicht verfügbar.

journal.window.shared.status.pane.visible.text.devices.are.adding.to.your.journal = Geräte fügen deinem Journal hinzu

journal.window.shared.status.pane.visible.text.hide.details = Details ausblenden

journal.window.shared.status.pane.visible.text.just.now = gerade eben

journal.window.shared.status.pane.visible.text.no.devices.are.running.the.solstone = noch läuft die solstone-App auf keinem Gerät. richte eins ein, um dein Journal zu starten.

journal.window.shared.status.pane.visible.text.no.messages.yet = noch keine Nachrichten

journal.window.shared.status.pane.visible.text.no.notifications.held.back = keine Benachrichtigungen zurückgehalten

journal.window.shared.status.pane.visible.text.no.notifications.yet = noch keine Benachrichtigungen

journal.window.shared.status.pane.visible.text.nothing.held.back.nothing.new = nichts zurückgehalten, nichts Neues

journal.window.shared.status.pane.visible.text.notifications = Benachrichtigungen

journal.window.shared.status.pane.visible.text.quiet.notifications = leise Benachrichtigungen

journal.window.shared.status.pane.visible.text.show.details = Details anzeigen

journal.window.shared.status.pane.visible.text.solstone.app.install.linux.packages = solstone.app/install#linux-packages

journal.window.shared.status.pane.visible.text.under.a.minute = unter einer Minute

journal.window.shared.status.pane.visible.text.unknown = unbekannt

journal.window.shared.status.pane.visible.text.update.status.unknown = Aktualisierungsstatus unbekannt

journal.window.shared.status.pane.visible.text.view.device.health = Gerätezustand ansehen →

journal.window.shared.status.pane.window.explanation.a.device.hasn.t.added.to = ein Gerät hat deinem Journal in letzter Zeit nichts hinzugefügt.

journal.window.shared.status.pane.window.explanation.a.device.needs.attention = ein Gerät braucht Aufmerksamkeit

journal.window.shared.status.pane.window.explanation.installed.from.a.package.updates.come = aus einem Paket installiert. Aktualisierungen kommen durchs Aktualisierungsprogramm deines Systems.

journal.window.shared.status.pane.window.explanation.installs.on.a.connection.that.isn = installiert über eine Verbindung, die nicht getaktet ist

journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.and = einmal am Tag suchen dein Journal und die solstone-App nach einer neuen Version und installieren sie im Hintergrund. dein Journal startet nie von selbst neu.

journal.window.shared.status.pane.window.explanation.once.a.day.your.journal.checks = einmal am Tag sucht dein Journal nach einer neuen Version und installiert sie im Hintergrund. es startet nie von selbst neu.

journal.window.shared.status.pane.window.explanation.update.your.journal.automatically = dein Journal automatisch aktualisieren?

journal.window.shared.status.pane.window.explanation.what.it.adds.isn.t.reaching = was es hinzufügt, erreicht dein Journal nicht.

journal.window.shared.websocket.label.a.device.hasn.t.added.recently = ein Gerät hat in letzter Zeit nichts hinzugefügt

journal.window.shared.websocket.label.a.device.needs.attention = ein Gerät braucht Aufmerksamkeit

journal.window.shared.websocket.label.attention = Achtung

journal.window.shared.websocket.label.can.t.confirm = lässt sich nicht bestätigen

journal.window.shared.websocket.label.connecting = wird verbunden

journal.window.shared.websocket.label.disconnected = getrennt

journal.window.shared.websocket.label.going.in = geht hinein

journal.window.shared.websocket.label.no.devices.connected = keine Geräte verbunden

journal.window.shared.websocket.label.reconnect.now = Jetzt neu verbinden

journal.window.shared.websocket.label.show.details = Details anzeigen

journal.window.shared.websocket.message.last.reconnect.attempt.failed = letzter Neuverbindungsversuch ist fehlgeschlagen.

journal.window.shared.websocket.message.live.updates.connected = Live-Aktualisierungen verbunden

journal.window.shared.websocket.message.reconnecting.some.features.may.be.delayed = wird neu verbunden. manche Funktionen können sich verzögern

journal.window.shared.websocket.reconnect.failed.history = letzter Wiederverbindungsversuch ist nicht gelungen. letzte Versuche: { $attempt_times }

journal.window.shared.websocket.reconnect.failed.no.history = der letzte Wiederverbindungsversuch ist nicht gelungen. noch keine Wiederverbindungsversuche aufgezeichnet.

journal.window.shared.websocket.title.connection.lost = Verbindung verloren

journal.window.shared.websocket.title.reconnected = wieder verbunden

journal.window.shell.connecting = wird verbunden…

journal.window.shell.error.pinned.must.be.a.boolean = pinned muss ein boolescher Wert sein

journal.window.shell.error.pins.request.must.be.an.object = pins-Anfrage muss ein Objekt sein

journal.window.shell.error.that.view.can.t.be.pinned = diese Ansicht lässt sich nicht anheften

journal.window.shell.error.the.pins.request.could.not.be.read = die pins-Anfrage konnte nicht gelesen werden

journal.window.shell.error.unknown.view = unbekannte Ansicht

journal.window.shell.error.view.must.be.a.string = Ansicht muss eine Zeichenkette sein

journal.window.shell.error.your.settings.couldn.t.be.read = deine Einstellungen konnten nicht gelesen werden.

journal.window.shell.guard.authorization.revoked = dieses gekoppelte Gerät ließ sich nicht verwenden, weil es widerrufen wurde.

journal.window.shell.guard.authorization.revoked.detail = gekoppeltes Gerät widerrufen

journal.window.shell.guard.authorization.unavailable.detail = Autorisierung des gekoppelten Geräts nicht verfügbar

journal.window.shell.guard.authorization.unavailable.mac = dein Journal konnte nicht prüfen, welche Geräte mit ihm gekoppelt sind. wenn das weiter geschieht, öffne das Admin-Terminal aus dem journal-Menü der Journal-App auf dem Mac, auf dem dein Journal läuft, und führe `{ $slot1 }` aus.

journal.window.shell.guard.authorization.unavailable.other = dein Journal konnte nicht prüfen, welche Geräte mit ihm gekoppelt sind. wenn das weiter passiert, führe `{ $slot1 }` auf dem Computer, auf dem dein Journal läuft, aus.

journal.window.shell.guard.form.denied = dein Journal hat diese Anfrage abgewiesen, weil sie nicht von den eigenen Seiten deines Journals gesendet wurde.

journal.window.shell.guard.form.detail = zustandsändernde Anfragen brauchen { $slot1 }

journal.window.shell.guard.host.denied = die Web-App deines Journals lässt sich nur unter { $slot1 }, { $slot2 } oder { $slot3 } auf dem Computer öffnen, auf dem sie läuft.

journal.window.shell.guard.host.detail = Host nicht erlaubt

journal.window.shell.guard.origin.detail = ursprungsübergreifende Anfrage

journal.window.shell.guard.site.denied = die Web-App deines Journals hat diese Anfrage abgewiesen, weil sie von einer anderen Website kam.

journal.window.shell.guard.site.detail = seitenübergreifende Anfrage

journal.window.shell.last.message = letzte Nachricht:

journal.window.shell.loading = wird geladen…

journal.window.shell.notifications.empty = noch keine Benachrichtigungen

journal.window.shell.notifications.enable = Browser-Benachrichtigungen aktivieren

journal.window.shell.notifications.label = Benachrichtigungen

journal.window.shell.notifications.quiet = leise Benachrichtigungen

journal.window.shell.notifications.quiet.explanation = Fehler von Hintergrunddiensten, die nicht als Benachrichtigungen angezeigt wurden.

journal.window.shell.notifications.recent = neueste Benachrichtigungen

journal.window.shell.pin.error.invalid = diese Einstellung konnte nicht gespeichert werden, weil ein Wert ungültig war.

journal.window.shell.pin.error.save = deine Einstellungen konnten nicht gespeichert werden.

journal.window.shell.presentation.banner = PRÄSENT — Umschalt+P zum Beenden

journal.window.shell.redirect.instruction = Du solltest automatisch zur Ziel-URL weitergeleitet werden: { $destination_link }. Falls nicht, klick den Link.

journal.window.shell.redirect.title = Wird weitergeleitet...

journal.window.shell.settings.read.failed = deine Einstellungen konnten nicht gelesen werden.

journal.window.shell.skip.to.workspace = zum Arbeitsbereich springen

journal.window.shell.state = Zustand:

journal.window.shell.status = Status

journal.window.shell.status.details = Details zum Status des Journals

journal.window.shell.status.silent.devices = { $count }{ $slot1 }Geräte haben in letzter Zeit nichts hinzugefügt

journal.window.shell.technical.details = technische Details

journal.window.shell.title = journal

journal.window.shell.update.link = so aktualisierst du dein Journal

journal.window.shell.uptime = Betriebszeit:

journal.window.shell.version = Version

journal.window.shell.version.unknown = unbekannt

journal.window.shell.views = Ansichten

journal.window.view.activities.date.count = { $quantity ->
        [one] 1 Aktivität
       *[other] { $count } Aktivitäten
    }

journal.window.view.activities.date.none = keine Aktivitäten

journal.window.view.activities.date.one = Aktivität

journal.window.view.activities.date.other = Aktivitäten

journal.window.view.activities.description = was du an jedem Tag getan hast, nach Facette sortiert

journal.window.view.activities.label = Aktivitäten

journal.window.view.activities.source = von deinem Journal aufgeschrieben, aus dem, was du geteilt hast

journal.window.view.agents.description = einen Agenten verbinden, der dein Journal liest und durchsucht, im Rahmen dessen, was du wählst

journal.window.view.agents.label = Agenten

journal.window.view.agents.source = Agenten, die du verbindest

journal.window.view.backup.description = eine verschlüsselte Kopie deines Journals, die nur du lesen kannst

journal.window.view.backup.label = Sicherung

journal.window.view.backup.source = dein eigener Speicher oder Speicher, den { $slot1 } betreibt

journal.window.view.body.date.count = { $quantity ->
        [one] 1 Messwert
       *[other] { $count } Messwerte
    }

journal.window.view.body.date.none = keine Messwerte

journal.window.view.body.date.one = Messwert

journal.window.view.body.date.other = Messwerte

journal.window.view.body.description = Schlaf, Trainings und Puls-Messwerte

journal.window.view.body.label = Körper

journal.window.view.body.source = aus dem, was du importierst

journal.window.view.curation.description = Änderungen, die dein Journal vorschlägt, zum Annehmen oder Überspringen

journal.window.view.curation.label = Vorschläge

journal.window.view.curation.source = von deinem Journal vorgeschlagen, während es arbeitet

journal.window.view.entities.description = Personen, Firmen, Projekte und Werkzeuge, die dein Journal behält, zum Bearbeiten, Zusammenführen oder Ausblenden

journal.window.view.entities.label = Entitäten

journal.window.view.entities.source = Namen, die dein Journal in dem, was es behält, gefunden hat

journal.window.view.health.description = ob dein Journal mithält: Verarbeitung, Geräte, Fehler

journal.window.view.health.label = Zustand

journal.window.view.health.source = die eigenen Prüfungen deines Journals

journal.window.view.home.description = heute und gestern, aufgeschrieben und alles, was dein Journal von dir braucht

journal.window.view.home.label = Start

journal.window.view.home.source = von deinem Journal aus allem darin geschrieben

journal.window.view.import.description = Chats, Kalender, Notizen, Audio und Trainings hereinholen

journal.window.view.import.label = Import

journal.window.view.import.source = Dateien, die du einbringst

journal.window.view.map.date.count = { $quantity ->
        [one] 1 Datensatz
       *[other] { $count } Datensätze
    }

journal.window.view.map.date.none = kein Ort

journal.window.view.map.date.one = Datensatz

journal.window.view.map.date.other = Datensätze

journal.window.view.map.description = wo du warst, Tag für Tag, und die Orte, die du benennst

journal.window.view.map.label = Karte

journal.window.view.map.source = von der solstone-App auf deinem Telefon, sobald der Ort dort an ist

journal.window.view.more.label = mehr

journal.window.view.network.description = ein Telefon oder einen Computer koppeln und wie deine Geräte dein Journal erreichen

journal.window.view.network.label = Netzwerk

journal.window.view.network.source = deine Geräte

journal.window.view.search.description = Wörter und Ausdrücke überall in deinem Journal finden

journal.window.view.search.label = Suche

journal.window.view.search.source = alles, was dein Journal behält

journal.window.view.settings.description = dein Profil, Schlüssel, Transkription, Speicher und Facetten

journal.window.view.settings.label = Einstellungen

journal.window.view.sources.description = wann deine Geräte zuletzt etwas zu deinem Journal hinzugefügt haben.

journal.window.view.sources.label = Quellen

journal.window.view.sources.source = deine gekoppelten Geräte

journal.window.view.speakers.date.count = { $quantity ->
        [one] 1 Segment
       *[other] { $count } Segmente
    }

journal.window.view.speakers.date.none = keine Segmente

journal.window.view.speakers.date.one = Segment

journal.window.view.speakers.date.other = Segmente

journal.window.view.speakers.description = die Stimmen in deinem Audio: bring deine bei, benenne andere, korrigiere, wer was gesagt hat

journal.window.view.speakers.label = Sprecher

journal.window.view.speakers.source = Stimmen im Audio, das du geteilt hast

journal.window.view.stats.description = wie viel dein Journal hält und wie viel Verarbeitung es geleistet hat

journal.window.view.stats.label = Statistiken

journal.window.view.stats.source = die eigenen Anzahlen deines Journals

journal.window.view.support.description = eine Meldung erstellen oder eine verfolgen, die du erstellt hast

journal.window.view.support.label = Support

journal.window.view.thinking.description = wo das Denken passiert, und jeder Lauf

journal.window.view.thinking.label = Denken

journal.window.view.thinking.source = auf dem Computer deines Journals, auf der von { $slot1 } betriebenen vertraulichen Verarbeitung oder mit deinem eigenen Modell

journal.window.view.transcripts.date.count = { $quantity ->
        [one] 1 Segment
       *[other] { $count } Segmente
    }

journal.window.view.transcripts.date.none = keine Segmente

journal.window.view.transcripts.date.one = Segment

journal.window.view.transcripts.date.other = Segmente

journal.window.view.transcripts.description = was gesagt wurde und auf dem Bildschirm, Moment für Moment, mit dem Audio

journal.window.view.transcripts.label = Transkripte

journal.window.view.transcripts.source = von deinem Mikrofon und Bildschirm über die solstone-App und aus Importen

journal.window.view.year.description = ein ganzes Jahr auf einmal: jeder Tag, und jeder Ort und jeder Name pro Woche

journal.window.view.year.label = Jahr

journal.window.view.year.source = gezählt aus dem, was dein Journal behält

journal.window.year.copy.days.days = Tage

journal.window.year.copy.days.lede.each.square.is.a.day.colored = jedes Quadrat ist ein Tag, gefärbt nach der Anzahl, die du unten wählst.

journal.window.year.copy.empty.action.see.your.sources = deine Quellen ansehen

journal.window.year.copy.empty.detail.once.your.journal.has.activities.places = sobald dein Journal Aktivitäten, Orte oder Namen für einen Tag hat, erscheint sein Jahr hier.

journal.window.year.copy.empty.nothing.here.yet = hier ist noch nichts.

journal.window.year.copy.error.this.didn.t.load = das wurde nicht geladen.

journal.window.year.copy.find.find.a.place.or.a.name = finde einen Ort oder einen Namen

journal.window.year.copy.layer.label.what.to.count = was gezählt wird

journal.window.year.copy.names.unavailable.names.aren.t.listed.your.journal = Namen sind nicht aufgeführt: dein Journal konnte nicht prüfen, welche auszulassen sind.

journal.window.year.copy.other.names.other.names = andere Namen

journal.window.year.copy.peek.hint.choose.a.row.or.a.week = wähle eine Zeile oder eine Woche darauf, um ihre Tage zu sehen.

journal.window.year.copy.places.places = Orte

journal.window.year.copy.places.unavailable.places.aren.t.listed.the.place = Orte sind nicht aufgeführt: die Ortsnamen, die mit deinem Journal kommen, konnten nicht gelesen werden.

journal.window.year.copy.retry.try.again = erneut versuchen

journal.window.year.copy.step.label.other.years = andere Jahre

journal.window.year.copy.switch.label.how.to.see.this.year = wie du dieses Jahr siehst

journal.window.year.copy.threads.lede.each.row.is.a.place.or = jede Zeile ist ein Ort oder ein Name, den dein Journal gefunden hat. jedes Quadrat ist eine Woche, gefärbt danach, an wie vielen Tagen dieser Woche der Ort oder der Name vorkommt. die Zeilen stehen in der Reihenfolge, in der sie zuerst erscheinen.

journal.window.year.copy.threads.threads = Stränge

journal.window.year.copy.title.year = Jahr

journal.window.year.copy.week.after.the.later.week.with.days.on = die spätere Woche mit Tagen in dieser Zeile

journal.window.year.copy.week.before.the.earlier.week.with.days.on = die frühere Woche mit Tagen in dieser Zeile

journal.window.year.days.empty = nichts dieser Art in { $year }.

journal.window.year.months.label.apr = Apr

journal.window.year.months.label.aug = Aug

journal.window.year.months.label.dec = Dez

journal.window.year.months.label.feb = Feb

journal.window.year.months.label.jan = Jan

journal.window.year.months.label.jul = Jul

journal.window.year.months.label.jun = Jun

journal.window.year.months.label.mar = Mär

journal.window.year.months.label.may = Mai

journal.window.year.months.label.nov = Nov

journal.window.year.months.label.oct = Okt

journal.window.year.months.label.sep = Sep

journal.window.year.source.or.date.unit.label.activities = Aktivitäten

journal.window.year.source.or.date.unit.label.activity = Aktivität

journal.window.year.source.or.date.unit.label.location = Ort

journal.window.year.source.or.date.unit.label.no.activities = keine Aktivitäten

journal.window.year.source.or.date.unit.label.no.location = kein Ort

journal.window.year.source.or.date.unit.label.record = Datensatz

journal.window.year.source.or.date.unit.label.records = Datensätze

journal.window.year.thread.cell.week.days = { $days_quantity ->
        [one] { $thread_name }, Woche von { $week_start }: { $days_count } Tag
       *[other] { $thread_name }, Woche von { $week_start }: { $days_count } Tage
    }

journal.window.year.thread.days.in.year = { $days_quantity ->
        [one] 1 Tag in { $year }, { $first_day }.
       *[other] { $days_count } Tage in { $year }, { $first_day } bis { $last_day }, über { $weeks_quantity ->
        [one] { $weeks_count } Woche.
       *[other] { $weeks_count } Wochen.
    }
    }

journal.window.year.thread.empty = keine Orte oder Namen in { $year }.

journal.window.year.thread.find.empty = nichts in { $year } hat diesen Namen.

journal.window.year.thread.group.name.count = { $total_quantity ->
        [one] { $total_count } Name
       *[other] { $total_count } Namen
    }

journal.window.year.thread.group.place.count = { $total_quantity ->
        [one] { $total_count } Ort
       *[other] { $total_count } Orte
    }

journal.window.year.thread.jump.to.first.day = bei { $first_day } beginnen

journal.window.year.thread.list.empty = nichts aufgeführt für { $year }.

journal.window.year.thread.week.day.links = { $days_quantity ->
        [one] { $days_count } Tag dieser Woche: { $day_links }
       *[other] { $days_count } Tage dieser Woche: { $day_links }
    }

journal.window.year.thread.week.range = Woche von { $start_day } bis { $end_day }

journal.window.year.type.labels.company.companies = Firmen

journal.window.year.type.labels.organization.organizations = Organisationen

journal.window.year.type.labels.person.people = Personen

journal.window.year.type.labels.place.named.places = benannte Orte

journal.window.year.type.labels.project.projects = Projekte

journal.window.year.type.labels.tool.tools = Werkzeuge

journal.window.year.view.empty = hier ist für { $year } nichts zu zeigen.

journal.window.year.year.invalid = dieses Jahr konnte nicht verwendet werden.

journal.window.year.year.invalid.detail = Ungültiges Jahr, YYYY erwartet
