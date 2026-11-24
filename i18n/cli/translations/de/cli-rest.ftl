journal.cli.backfill.committed = ÜBERNOMMEN

journal.cli.backfill.day.missing = Tag { $day } wurde im Journal nicht gefunden

journal.cli.backfill.days = Chroniktage konnten nicht aufgelistet werden: { $error }

journal.cli.backfill.flags.conflict = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: Fehler: Argument { $flag }: nicht zulässig mit Argument { $previous }

journal.cli.backfill.flags.missing = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: Fehler: Argument { $day_flag }: genau ein Argument erwartet

journal.cli.backfill.flags.unknown = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: Fehler: unbekannte Argumente: { $argument }

journal.cli.backfill.help = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }Leere Verarbeitungsdatensätze auf feststeckende, nur aus der Kopfzeile bestehende native{ $line_break }{ $handlers }-Ausgaben auffüllen{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $day_option }{ $gap_6 }Tag im Format { $date_format }; standardmäßig alle Tage{ $line_break }{ $indent_2 }{ $commit_option }{ $gap_7 }Geeignete { $jsonl_format }-Kopfzeilen mit leeren Verarbeitungsdatensätzen neu schreiben{ $line_break }{ $indent_2 }{ $dry_run_option }{ $gap_6 }Geeignete Ausgaben in der Vorschau anzeigen, ohne Änderungen zu schreiben{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }ausführliche Ausgabe einschalten{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Debug-Protokollierung einschalten

journal.cli.backfill.identity.ambiguous = benanntes Datenstrom-Verzeichnis "{ $default_stream }" kann nicht als Datensatzidentität buchstabiert werden: { $path }

journal.cli.backfill.identity.duplicate = mehrere Segmente teilen sich Datenstrom { $stream } und Schlüssel { $key }

journal.cli.backfill.identity.encoding = der Segmentpfad ist nicht in { $encoding } darstellbar: { $path }

journal.cli.backfill.invalid.day = ein Tag im Format { $date_format } wurde erwartet

journal.cli.backfill.marker = Gestempelt wurde { $path }, aber Tag { $day } konnte nicht als aktualisiert markiert werden: { $error }

journal.cli.backfill.preview = PROBELAUF (keine Änderungen geschrieben)

journal.cli.backfill.segment.directory = Segmentverzeichnis { $path } konnte nicht aufgelistet werden: { $error }

journal.cli.backfill.segments = Segmente für Tag { $day } konnten nicht aufgelistet werden: { $error }

journal.cli.backfill.stamp.changed = { $path } konnte nicht gestempelt werden: Datei nach der Klassifizierung geändert

journal.cli.backfill.stamp.read = { $path } konnte nicht gestempelt werden: Datei konnte nicht erneut gelesen werden: { $error }

journal.cli.backfill.stamp.write = { $path } konnte nicht gestempelt werden: das Schreiben ist fehlgeschlagen: { $error }

journal.cli.backfill.stream.directory = Datenstrom-Verzeichnis { $path } konnte nicht aufgelistet werden: kein Verzeichnis

journal.cli.backfill.stream.encoding = der Segment-Datenstrom ist nicht in { $encoding } darstellbar: { $path }

journal.cli.brain.arguments.invalid = { $command }: Fehler: ungültige Argumente

journal.cli.brain.busy = Denken beschäftigt: eine Prüfung läuft bereits

journal.cli.brain.check.config = die Prüfung des Denkens ist fehlgeschlagen: die Journal-Konfiguration konnte nicht gelesen werden: { $detail }

journal.cli.brain.check.failure = die Prüfung des Denkens ist fehlgeschlagen: { $detail }

journal.cli.brain.check.fingerprint.options = erwarteter aktiver Fingerabdruck und erwartete Abwesenheit schließen sich gegenseitig aus

journal.cli.brain.check.key.missing = der Anbieter braucht einen Schlüssel, und dein Journal konnte nicht bestätigen, dass das Ergebnis gespeichert wurde.

journal.cli.brain.check.key.rejected = der Anbieter hat diesen Schlüssel abgelehnt, und dein Journal konnte nicht bestätigen, dass das Ergebnis gespeichert wurde.

journal.cli.brain.check.open.directory = ein Nachfahre im flachen Verzeichnis konnte für { $path } nicht geöffnet werden: { $reason }

journal.cli.brain.check.random = sichere Zufallsbytes konnten nicht erzeugt werden

journal.cli.brain.heading.blocked = Denken blockiert: { $reason }

journal.cli.brain.heading.blocked.component = Denken blockiert: { $reason } ({ $component })

journal.cli.brain.heading.checking = Denken wird geprüft: { $reason }

journal.cli.brain.heading.checking.component = Denken wird geprüft: { $reason } ({ $component })

journal.cli.brain.heading.other = Denken { $state }: { $reason }

journal.cli.brain.heading.other.component = Denken { $state }: { $reason } ({ $component })

journal.cli.brain.heading.unhealthy = Denken nicht in Ordnung: { $reason }

journal.cli.brain.heading.unhealthy.component = Denken nicht in Ordnung: { $reason } ({ $component })

journal.cli.brain.heading.unknown = Denken unbekannt: { $reason }

journal.cli.brain.heading.unknown.component = Denken unbekannt: { $reason } ({ $component })

journal.cli.brain.help.refresh = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }{ $json_format } statt einfacher Ausgabe ausgeben{ $line_break }{ $syntax_continuation_7 }{ $line_break }{ $syntax_continuation_8 }{ $line_break }{ $syntax_continuation_9 }

journal.cli.brain.help.renew = Verwendung: { $syntax }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }diese Hilfe anzeigen und beenden

journal.cli.brain.help.root = Verwendung: { $syntax }{ $line_break }{ $line_break }Status des aktiven Denkens und CLI für begrenzte Auffrischung.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $status }Status des aktiven Denkens anzeigen{ $line_break }    { $refresh }Eine begrenzte Prüfung des aktiven Denkens ausführen{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_10 }diese Hilfe anzeigen und beenden

journal.cli.brain.help.status = Verwendung: { $syntax }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }{ $json_format } statt einfacher Ausgabe ausgeben

journal.cli.brain.hold.capacity = die Kapazität der Zugangsdatenhaltung ist nicht verfügbar

journal.cli.brain.hold.invalid.provider = nicht unterstützter Anbieter der Zugangsdatenhaltung

journal.cli.brain.hold.root.changed = zugelassene Wurzel des Journals geändert

journal.cli.brain.hold.superseded = die Validierung der Zugangsdaten wurde abgelöst

journal.cli.brain.hold.unavailable = der Zustand der Zugangsdatenhaltung ist nicht verfügbar

journal.cli.brain.ready.byo = Denken bereit: dein eigenes Modell

journal.cli.brain.ready.byo.checked = Denken bereit: dein eigenes Modell, geprüft vor { $age }

journal.cli.brain.ready.byo.model = Denken bereit: dein eigenes Modell { $model }

journal.cli.brain.ready.byo.model.checked = Denken bereit: dein eigenes Modell { $model }, geprüft vor { $age }

journal.cli.brain.ready.local = Denken bereit: lokales Denken

journal.cli.brain.ready.local.checked = Denken bereit: lokales Denken, geprüft vor { $age }

journal.cli.brain.ready.local.model = Denken bereit: lokales Denken { $model }

journal.cli.brain.ready.local.model.checked = Denken bereit: lokales Denken { $model }, geprüft vor { $age }

journal.cli.brain.ready.other = Denken bereit: { $lane }

journal.cli.brain.ready.other.checked = Denken bereit: { $lane }, geprüft vor { $age }

journal.cli.brain.ready.other.model = Denken bereit: { $lane } { $model }

journal.cli.brain.ready.other.model.checked = Denken bereit: { $lane } { $model }, geprüft vor { $age }

journal.cli.brain.ready.spp = Denken bereit: vertrauliche Verarbeitung

journal.cli.brain.ready.spp.checked = Denken bereit: vertrauliche Verarbeitung, geprüft vor { $age }

journal.cli.brain.ready.spp.model = Denken bereit: vertrauliche Verarbeitung { $model }

journal.cli.brain.ready.spp.model.checked = Denken bereit: vertrauliche Verarbeitung { $model }, geprüft vor { $age }

journal.cli.brain.ready.unknown = Denken bereit: unbekannt

journal.cli.brain.ready.unknown.checked = Denken bereit: unbekannt, geprüft vor { $age }

journal.cli.brain.ready.unknown.model = Denken bereit: unbekannt { $model }

journal.cli.brain.ready.unknown.model.checked = Denken bereit: unbekannt { $model }, geprüft vor { $age }

journal.cli.brain.settlement.fingerprint.changed = geschützter Fingerabdruckschlüssel nach der Validierung geändert

journal.cli.brain.settlement.publication.changed = Zugangsdaten-Validierung nach der Veröffentlichung geändert

journal.cli.brain.settlement.root.changed = zugelassener Pfad des Journals geändert

journal.cli.brain.settlement.selection.changed = Zugangsdaten-Auswahl während der Auffrischung geändert

journal.cli.brain.settlement.unconfirmed = die Veröffentlichung der Zugangsdaten-Validierung wurde nicht bestätigt

journal.cli.brain.settlement.validation.changed = Zugangsdaten-Validierung nach der Auffrischung geändert

journal.cli.brain.usage = Verwendung: { $syntax }

journal.cli.brain.validation.checking.aggregate.requires.matching.checking.marker = prüfendes Aggregat erfordert eine passende Prüfmarkierung

journal.cli.brain.validation.checking.reason.is.not.valid.component.evidence = Prüfgrund ist kein gültiger Komponenten-Nachweis

journal.cli.brain.validation.component.status.does.not.match.reason.aggregate = Komponentenstatus stimmt nicht mit dem Grund-Aggregat überein

journal.cli.brain.validation.detail = { $field }: { $reason }

journal.cli.brain.validation.diagnostic.enum.value.not.allowed = diagnostischer Enum-Wert nicht zulässig

journal.cli.brain.validation.diagnostic.key.not.allowed = diagnostischer Schlüssel nicht zulässig

journal.cli.brain.validation.diagnostic.value.must.be.an.enum.string = diagnostischer Wert muss eine Enum-Zeichenkette sein

journal.cli.brain.validation.does.not.match.aggregate.state = stimmt nicht mit { $token_aggregate_state } überein

journal.cli.brain.validation.expected.sha256.hex.string = eine { $token_sha_256 }-Hex-Zeichenkette wurde erwartet

journal.cli.brain.validation.is.required.when.status.is.ok = ist erforderlich, wenn der Status ok ist

journal.cli.brain.validation.missing.lane.applicable.evidence.without.higher.priority.cause = fehlender Lane-anwendbarer Nachweis ohne Ursache höherer Priorität

journal.cli.brain.validation.must.be.a.non.negative.integer = muss eine nichtnegative Ganzzahl sein

journal.cli.brain.validation.must.be.a.sha256.hex.digest = muss ein { $token_sha_256 }-Hex-Digest sein

journal.cli.brain.validation.must.be.a.string = muss eine Zeichenkette sein

journal.cli.brain.validation.must.be.a.string.or.null = muss eine Zeichenkette oder null sein

journal.cli.brain.validation.must.be.an.integer = muss eine Ganzzahl sein

journal.cli.brain.validation.must.be.an.iso8601.timestamp = muss ein { $token_iso_8601 }-Zeitstempel sein

journal.cli.brain.validation.must.be.an.object = muss ein Objekt sein

journal.cli.brain.validation.must.be.evidence.recordable = muss als Nachweis erfassbar sein

journal.cli.brain.validation.must.be.non.empty = muss nicht-leer sein

journal.cli.brain.validation.must.be.null.when.aggregate.state.is.ready = muss null sein, wenn { $token_aggregate_state } bereit ist

journal.cli.brain.validation.must.be.provider.key.invalid = muss { $token_provider_key_invalid } sein

journal.cli.brain.validation.must.be.set.when.aggregate.state.is.not.ready = muss gesetzt sein, wenn { $token_aggregate_state } nicht bereit ist

journal.cli.brain.validation.non.ok.evidence.requires.reason = nicht-ok-Nachweis erfordert einen Grund

journal.cli.brain.validation.not.attempted.is.only.valid.for.generate = { $token_not_attempted } ist nur für das Erzeugen gültig

journal.cli.brain.validation.not.attempted.reason.must.repeat.causal.prerequisite.reason = { $token_not_attempted }-Grund muss den ursächlichen Voraussetzungsgrund wiederholen

journal.cli.brain.validation.not.attempted.requires.a.non.ok.prerequisite = { $token_not_attempted } erfordert eine nicht-ok-Voraussetzung

journal.cli.brain.validation.ok.evidence.requires.null.reason = ok-Nachweis erfordert den Grund null

journal.cli.brain.validation.prerequisite.renewal.requires.ok.or.declared.failure = Erneuerung der Voraussetzung erfordert ok oder einen deklarierten Fehlschlag

journal.cli.brain.validation.reason.not.allowed.for.evidence.component = Grund für die Nachweis-Komponente nicht zulässig

journal.cli.brain.validation.record.aggregate.reason.does.not.match.evidence = Datensatz-Aggregat/Grund stimmt nicht mit dem Nachweis überein

journal.cli.brain.validation.unknown.field = unbekanntes Feld

journal.cli.brain.validation.unknown.top.level.field = unbekanntes Feld der obersten Ebene

journal.cli.brain.validation.unknown.value = unbekannter Wert

journal.cli.brain.validation.unsupported.binding.version = nicht unterstützte Bindungsversion

journal.cli.brain.validation.unsupported.schema.version = nicht unterstützte Schemaversion

journal.cli.brain.writer.active.brain.fingerprint.is.unavailable = der Fingerabdruck des aktiven Denkens ist nicht verfügbar

journal.cli.brain.writer.brain.abandon.reason.is.not.recordable.evidence = der Abbruchgrund des Denkens ist kein erfassbarer Nachweis

journal.cli.brain.writer.brain.fingerprint.changed = Fingerabdruck des Denkens geändert

journal.cli.brain.writer.brain.fingerprint.key.is.unavailable = der Fingerabdruckschlüssel des Denkens ist nicht verfügbar

journal.cli.brain.writer.brain.lane.changed = Lane des Denkens geändert

journal.cli.brain.writer.brain.prerequisite.evidence.is.unsafe = der Voraussetzungsnachweis des Denkens ist unsicher

journal.cli.brain.writer.brain.record.changed.during.inspection = Datensatz des Denkens während der Inspektion geändert

journal.cli.brain.writer.brain.refresh.checking.marker.is.absent = die Prüfmarkierung der Auffrischung des Denkens ist nicht vorhanden

journal.cli.brain.writer.brain.refresh.permit.expired = Genehmigung der Auffrischung des Denkens abgelaufen

journal.cli.brain.writer.brain.refresh.record.revision.changed = Revision des Auffrischungsdatensatzes des Denkens geändert

journal.cli.brain.writer.brain.refresh.revision.changed = Revision der Auffrischung des Denkens geändert

journal.cli.brain.writer.brain.refresh.run.id.changed = Lauf-ID der Auffrischung des Denkens geändert

journal.cli.brain.writer.brain.runtime.failure.marker.changed = Fehler-Markierung der Laufzeitumgebung des Denkens geändert

journal.cli.brain.writer.configuration.rejection.changed.after.publication = Konfigurationsablehnung nach der Veröffentlichung geändert

journal.cli.brain.writer.credential.authority.changed.during.inspection = Zugangsdaten-Autorität während der Inspektion geändert

journal.cli.brain.writer.credential.authority.changed.during.observation = Zugangsdaten-Autorität während der Beobachtung geändert

journal.cli.brain.writer.credential.authority.is.not.an.object = die Zugangsdaten-Autorität ist kein Objekt

journal.cli.brain.writer.credential.authority.is.unreadable = die Zugangsdaten-Autorität ist nicht lesbar

journal.cli.brain.writer.credential.binding.authority.could.not.be.read.error = die Autorität der Zugangsdaten-Bindung konnte nicht gelesen werden: { $error }

journal.cli.brain.writer.credential.hold.authority.changed = Autorität der Zugangsdatenhaltung geändert

journal.cli.brain.writer.credential.hold.authority.is.not.an.object = die Autorität der Zugangsdatenhaltung ist kein Objekt

journal.cli.brain.writer.credential.hold.authority.is.unreadable.error = die Autorität der Zugangsdatenhaltung ist nicht lesbar: { $error }

journal.cli.brain.writer.credential.observation.authority.is.not.an.object = die Autorität der Zugangsdaten-Beobachtung ist kein Objekt

journal.cli.brain.writer.credential.observation.authority.is.unreadable = die Autorität der Zugangsdaten-Beobachtung ist nicht lesbar

journal.cli.brain.writer.credential.observation.changed.during.refresh = Zugangsdaten-Beobachtung während der Auffrischung geändert

journal.cli.brain.writer.credential.observation.changed.during.work = Zugangsdaten-Beobachtung während der Arbeit geändert

journal.cli.brain.writer.credential.observation.identity.is.invalid = die Identität der Zugangsdaten-Beobachtung ist ungültig

journal.cli.brain.writer.credential.observation.lacks.original.binding = die Zugangsdaten-Beobachtung hat keine ursprüngliche Bindung

journal.cli.brain.writer.credential.observations.are.not.an.object = die Zugangsdaten-Beobachtungen sind kein Objekt

journal.cli.brain.writer.credential.process.state.changed.during.inspection = Prozesszustand der Zugangsdaten während der Inspektion geändert

journal.cli.brain.writer.credential.publication.reservation.is.absent = die Reservierung der Zugangsdaten-Veröffentlichung ist nicht vorhanden

journal.cli.brain.writer.credential.rejection.changed.after.publication = Zugangsdaten-Ablehnung nach der Veröffentlichung geändert

journal.cli.brain.writer.credential.rejection.changed.during.refresh = Zugangsdaten-Ablehnung während der Auffrischung geändert

journal.cli.brain.writer.credential.rejection.changed.during.validation = Zugangsdaten-Ablehnung während der Validierung geändert

journal.cli.brain.writer.field.must.be.an.object = { $field } muss ein Objekt sein

journal.cli.brain.writer.protected.fingerprint.key.changed.after.rejection = geschützter Fingerabdruckschlüssel nach der Ablehnung geändert

journal.cli.brain.writer.protected.fingerprint.key.changed.during.admission = geschützter Fingerabdruckschlüssel während der Zulassung geändert

journal.cli.brain.writer.protected.fingerprint.key.changed.during.inspection = geschützter Fingerabdruckschlüssel während der Inspektion geändert

journal.cli.brain.writer.protected.fingerprint.key.changed.during.validation = geschützter Fingerabdruckschlüssel während der Validierung geändert

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.a.credential.binding.exists = der geschützte Fingerabdruckschlüssel ist nicht vorhanden, während eine Zugangsdaten-Bindung besteht

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.credential.authority.exists = der geschützte Fingerabdruckschlüssel ist nicht vorhanden, während eine Zugangsdaten-Autorität besteht

journal.cli.brain.writer.protected.fingerprint.key.is.unavailable = der geschützte Fingerabdruckschlüssel ist nicht verfügbar

journal.cli.brain.writer.reason.is.not.recordable.evidence = der Grund ist kein erfassbarer Nachweis

journal.cli.brain.writer.rejected.refresh.receipt.does.not.match.its.probe = abgelehnte Auffrischungsquittung stimmt nicht mit ihrer Sonde überein

journal.cli.brain.writer.rejection.receipt.root.provider.changed = Wurzel/Anbieter der Ablehnungsquittung geändert

journal.cli.brain.writer.settled.credential.observation.changed.during.work = festgeschriebene Zugangsdaten-Beobachtung während der Arbeit geändert

journal.cli.brain.writer.validation.result.does.not.match.its.credential = Validierungsergebnis stimmt nicht mit seinen Zugangsdaten überein

journal.cli.brain.writer.validation.result.is.not.an.object = das Validierungsergebnis ist kein Objekt

journal.cli.depict.admission = die Zulassung des Journal-Prozesses ist fehlgeschlagen: { $detail }

journal.cli.depict.admission.binding.mismatch = Nichtübereinstimmung der Bindung der Startinstallation

journal.cli.depict.admission.birth.invalid = { $linux }-Prozessstart hat ein fehlerhaftes { $boot }

journal.cli.depict.admission.birth.position = { $linux }-Prozessstart hat ein fehlerhaftes { $boot } in Zeile { $line }, Spalte { $column }

journal.cli.depict.admission.byte.count = die Start-Pipe hat eine ungültige Byteanzahl zurückgegeben

journal.cli.depict.admission.cleanup.pending = die Bereinigung des vorherigen Starts { $io } bleibt unvollständig

journal.cli.depict.admission.commit.mismatch = Nichtübereinstimmung der Übernahme bei der Startzulassung

journal.cli.depict.admission.deadline.elapsed = die Startfrist ist verstrichen

journal.cli.depict.admission.deadline.invalid = abgelaufene oder ungültige { $windows }-Startfrist

journal.cli.depict.admission.descriptor.encoding = nicht-{ $unicode } { $windows }-Startdeskriptor

journal.cli.depict.admission.descriptor.size = übergroßer { $windows }-Startdeskriptor

journal.cli.depict.admission.endpoint = ungültiger Start-endpoint

journal.cli.depict.admission.entry.hosted = installierte Aufgabe am gehosteten Eintrag

journal.cli.depict.admission.entry.mismatch = Startvariante oder installierte Aktion stimmt nicht überein

journal.cli.depict.admission.frame.size = ungültige Startframe-Größe

journal.cli.depict.admission.grant.access = Zugriff oder Typ des Start-Handles stimmt nicht mit seiner Gewährung überein

journal.cli.depict.admission.grant.disk = die Start-Gewährung ist keine Datenträgerdatei

journal.cli.depict.admission.grant.duplicate = doppeltes Start-Handle oder doppelte Gewährungsart

journal.cli.depict.admission.grant.ordinary = die Start-Gewährung ist keine gewöhnliche Datenträgerdatei

journal.cli.depict.admission.guard.duplicate = Installationswächter: doppelter Identitätswächter { $field }

journal.cli.depict.admission.guard.encoding = nicht-{ $unicode } Installationswächter

journal.cli.depict.admission.guard.invalid = Installationswächter: ungültiger Identitätswächter { $field }

journal.cli.depict.admission.guard.mismatch = das Tupel des Startwächters unterscheidet sich vom geerbten Wächter

journal.cli.depict.admission.guard.missing = Installationswächter: die Felder des Identitätswächters sind unvollständig

journal.cli.depict.admission.handle.invalid = ungültiges übertragenes Handle

journal.cli.depict.admission.identity.io.local.folder = bekannten Ordner { $local_data } auflösen: { $error }

journal.cli.depict.admission.identity.io.marker.clone = Adoptionsmarkierung klonen: { $error }

journal.cli.depict.admission.identity.io.marker.read = Adoptionsmarkierung lesen: { $error }

journal.cli.depict.admission.identity.io.marker.stat = stat für die Adoptionsmarkierung: { $error }

journal.cli.depict.admission.identity.io.namespace.read = Namensraumverzeichnis lesen: { $error }

journal.cli.depict.admission.identity.io.owner.open = Besitzerverzeichnis öffnen: { $error }

journal.cli.depict.admission.identity.io.record.read = Identitätsdatensatz lesen: { $error }

journal.cli.depict.admission.identity.io.record.stat = stat für den Identitätsdatensatz: { $error }

journal.cli.depict.admission.identity.io.root = Wurzel kanonisieren: { $error }

journal.cli.depict.admission.identity.io.storage.attrs = Speicherattribute abfragen: { $error }

journal.cli.depict.admission.identity.io.storage.file = Speicherdatei öffnen: { $error }

journal.cli.depict.admission.identity.io.storage.identity = Speicheridentität abfragen: { $error }

journal.cli.depict.admission.identity.io.storage.lock = Identitätsspeicher sperren: { $error }

journal.cli.depict.admission.identity.io.storage.open = Speicherverzeichnis öffnen: { $error }

journal.cli.depict.admission.identity.local.encoding = { $local_data } bekannter Ordner ist nicht { $utf }-gültig

journal.cli.depict.admission.identity.marker.content = die Adoptionsmarkierung hat ungültigen Inhalt

journal.cli.depict.admission.identity.marker.missing = der übernommene Datensatz hat keine Adoptionsmarkierung

journal.cli.depict.admission.identity.marker.orphan = verwaiste Adoptionsmarkierung

journal.cli.depict.admission.identity.marker.size = die Adoptionsmarkierung hat eine ungültige Größe

journal.cli.depict.admission.identity.namespace.child = unbekanntes untergeordnetes Element des Namensraums

journal.cli.depict.admission.identity.owner.poisoned = der Besitzerkoordinator ist vergiftet

journal.cli.depict.admission.identity.path.device = { $windows }-Verbatim- und Gerätepfade werden nicht akzeptiert

journal.cli.depict.admission.identity.path.encoding = { $windows }-Pfad ist nicht { $utf }-gültig

journal.cli.depict.admission.identity.path.escape = { $windows }-Pfad verlässt seine absolute Wurzel

journal.cli.depict.admission.identity.path.nul = { $windows }-Pfad enthält im Inneren { $nul }

journal.cli.depict.admission.identity.path.relative = { $windows }-Pfad muss vollständig qualifiziert sein

journal.cli.depict.admission.identity.path.server = { $windows } { $unc }-Pfad hat keinen Server

journal.cli.depict.admission.identity.path.share = { $windows } { $unc }-Pfad hat keine Freigabe

journal.cli.depict.admission.identity.path.size = der Pfad überschreitet { $limit } Byte

journal.cli.depict.admission.identity.prepared = der Datensatz der Installationsidentität ist vorbereitet

journal.cli.depict.admission.identity.record.missing = der Namensraumdatensatz fehlt

journal.cli.depict.admission.identity.record.size = der Identitätsdatensatz überschreitet die Grenzen

journal.cli.depict.admission.identity.root.mismatch = der Datensatz bindet die angeforderte Wurzel nicht

journal.cli.depict.admission.identity.storage.directory = der Speicherpfad ist kein Verzeichnis

journal.cli.depict.admission.identity.storage.file = der Speicherpfad ist keine reguläre Datei

journal.cli.depict.admission.identity.storage.reparse = der Speicherpfad ist ein Reparsepunkt

journal.cli.depict.admission.identity.tombstoned = der Datensatz der Installationsidentität ist als Tombstone markiert

journal.cli.depict.admission.nonce = ungültige Start-endpoint-Nonce

journal.cli.depict.admission.obsolete = obsolete { $windows }-Startmarkierung

journal.cli.depict.admission.offer.mismatch = die Herkunft des Startangebots stimmt nicht überein

journal.cli.depict.admission.parent.mismatch = der Startserver ist nicht genau das direkte übergeordnete Element

journal.cli.depict.admission.parent.provenance = ungültige Herkunft des übergeordneten Starts

journal.cli.depict.admission.peer.unverified = die Gegenstelle der Start-Pipe ist nicht nachweislich aktiv

journal.cli.depict.admission.process.creation = für den aktuellen { $windows }-Prozess ist { $filetime } der Erstellung nicht verfügbar

journal.cli.depict.admission.process.signaled = der aktuelle { $windows }-Prozess ist signalisiert

journal.cli.depict.admission.provenance = ungültige Startherkunft

journal.cli.depict.admission.record.ascii = der Identitätsdatensatz ist nicht { $ascii }

journal.cli.depict.admission.record.checksum = die Prüfsumme des Identitätsdatensatzes stimmt nicht überein

journal.cli.depict.admission.record.field = ungültiger Identitätsdatensatz { $field }

journal.cli.depict.admission.record.lines = der Identitätsdatensatz ist nicht genau acht { $linefeed }-Zeilen

journal.cli.depict.admission.record.size = der Identitätsdatensatz überschreitet { $size }

journal.cli.depict.admission.root.missing = die Startprogrammdatei hat keine Installationswurzel

journal.cli.depict.admission.server.mismatch = die Identität des Start-Pipe-Servers stimmt nicht überein

journal.cli.depict.admission.snapshot.absent = der aktuelle Prozess ist im übergeordneten Abbild nicht vorhanden

journal.cli.depict.admission.task.provenance = ungültige Herkunft der installierten Aufgabe

journal.cli.depict.admission.wait.complete = das Warten auf Start-{ $io } ist nicht gelungen: { $error }; der Abbruch von { $io } ist abgeschlossen

journal.cli.depict.admission.wait.deadline.complete = die Startfrist ist abgelaufen; der Abbruch von { $io } ist abgeschlossen

journal.cli.depict.admission.wait.deadline.incomplete = die Startfrist ist abgelaufen; die Bereinigung von { $io } ist unvollständig und wurde behalten

journal.cli.depict.admission.wait.incomplete = das Warten auf Start-{ $io } ist nicht gelungen: { $error }; die Bereinigung von { $io } ist unvollständig und wurde behalten

journal.cli.depict.detection.unavailable = Objekterkennung ist nicht verfügbar. Führe `{ $command }` aus, um die { $engine }-Ressourcen zu prüfen oder zu reparieren.

journal.cli.depict.detection.unavailable.windows = Objekterkennung ist nicht verfügbar. repariere dein Journal oder installiere es neu.

journal.cli.depict.help = Verwendung: { $syntax }

journal.cli.depict.metadata.invalid = natives depict: { $variable } { $format } ist ungültig

journal.cli.describe.cancelled = describe abgebrochen

journal.cli.describe.decode.failed = das Video konnte nicht dekodiert werden: { $path }

journal.cli.describe.deferred = describe zurückgestellt

journal.cli.describe.deferred.reason = describe zurückgestellt: { $reason }

journal.cli.describe.faces.skipped = Gesichtsanreicherung übersprungen: { $kind }

journal.cli.describe.file.encoding = der Videopfad muss einen { $encoding }-Dateinamen haben

journal.cli.describe.output.encode = die Ausgabe konnte nicht kodiert werden: { $error }

journal.cli.describe.output.write = die Ausgabe konnte nicht geschrieben werden: { $error }

journal.cli.describe.path.config = der Journalpfad konnte nicht gelesen werden: { $reason }

journal.cli.describe.path.home = das Home-Verzeichnis konnte nicht aufgelöst werden: { $reason }

journal.cli.describe.pipeline.flat.bound = der gebundene Verzeichnisname existiert und ist kein Verzeichnis

journal.cli.describe.pipeline.flat.create.flat.directory.child = das untergeordnete Element des flachen Verzeichnisses konnte für { $path } nicht angelegt werden: { $detail }

journal.cli.describe.pipeline.flat.normal = der Name muss genau eine normale Komponente sein

journal.cli.describe.pipeline.flat.read.flat.directory.identity = die Identität des flachen Verzeichnisses konnte für { $path } nicht gelesen werden: { $detail }

journal.cli.describe.pipeline.flat.relative = der Pfad muss eine nicht leere Folge normaler portabler Komponenten sein

journal.cli.describe.pipeline.flat.revalidate.flat.directory = das flache Verzeichnis konnte für { $path } nicht erneut geprüft werden: { $detail }

journal.cli.describe.pipeline.flat.stat.flat.directory.descendant = stat für den Nachfahren des flachen Verzeichnisses ist für { $path } nicht gelungen: { $detail }

journal.cli.describe.pipeline.flat.stat.opened.flat.directory.descendant = stat für den geöffneten Nachfahren des flachen Verzeichnisses ist für { $path } nicht gelungen: { $detail }

journal.cli.describe.pipeline.flat.utf8 = der Name ist kein gültiger { $encoding }-Text

journal.cli.describe.pipeline.frames.failed = alle Anfragen für describe-Einzelbilder sind nicht gelungen

journal.cli.describe.pipeline.install.observe.installed.destination.after.move = { $path }: installiertes Ziel nach dem Verschieben beobachten: { $detail }

journal.cli.describe.pipeline.install.reconcile.install.after.failed.move = { $path }: Installation nach fehlgeschlagenem Verschieben in Einklang bringen: { $detail }

journal.cli.describe.pipeline.install.revalidate.install.paths.after.move = { $path }: Installationspfade nach dem Verschieben erneut prüfen: { $detail }

journal.cli.describe.pipeline.jobs = { $flag } muss positiv sein

journal.cli.describe.pipeline.merge.frame = Einzelbild zum Zusammenführen fehlt

journal.cli.describe.pipeline.metadata.object = { $segment_meta } muss ein Objekt sein

journal.cli.describe.pipeline.ocr.coverage = die Bildschirm-{ $ocr }-Abdeckung ist unvollständig; private Einzelbilddiagnosen zeichnen die Lücke auf

journal.cli.describe.pipeline.ocr.publication = die Bildschirm-{ $ocr }-Veröffentlichung ist nicht gelungen: { $kind }

journal.cli.describe.pipeline.ocr.retire = das veraltete Bildschirm-{ $ocr }-Aggregat konnte nicht stillgelegt werden: { $kind }

journal.cli.describe.pipeline.parent = das Video hat kein übergeordnetes Element

journal.cli.describe.pipeline.progress = das describe-Einzelbildfenster hat keinen Fortschritt gemacht

journal.cli.describe.pipeline.publish.anchor.is.not.a.mount.manager.volume.guid.path = der Anker ist kein { $manager }-Volume-{ $guid }-Pfad

journal.cli.describe.pipeline.publish.changed = die vorbereitete Veröffentlichung hat sich geändert

journal.cli.describe.pipeline.publish.component = das Blatt der Veröffentlichung ist keine einfache Komponente

journal.cli.describe.pipeline.publish.current.directory.is.not.valid.utf = das aktuelle Verzeichnis ist nicht { $encoding }-gültig

journal.cli.describe.pipeline.publish.destination = das Ziel der Veröffentlichung ist keine einfache Datei

journal.cli.describe.pipeline.publish.destination.already.identifies.the.source.file = das Ziel identifiziert bereits die Ursprungsdatei

journal.cli.describe.pipeline.publish.destination.changed.during.publication = das Ziel hat sich während der Veröffentlichung geändert

journal.cli.describe.pipeline.publish.destination.has.no.normal.name = das Ziel hat keinen normalen Namen

journal.cli.describe.pipeline.publish.destination.is.a.reparse.point = das Ziel ist ein Reparsepunkt

journal.cli.describe.pipeline.publish.destination.is.not.a.regular.file = das Ziel ist keine reguläre Datei

journal.cli.describe.pipeline.publish.identity = die Identität der vorbereiteten Veröffentlichung hat sich geändert

journal.cli.describe.pipeline.publish.identity.value.out.of.range = der Identitätswert liegt außerhalb des Bereichs

journal.cli.describe.pipeline.publish.install.publication.exhausted = die Veröffentlichung der Installation ist ausgeschöpft

journal.cli.describe.pipeline.publish.leaf = Blatt der Veröffentlichung fehlt

journal.cli.describe.pipeline.publish.mode.exceeds.o = der Modus überschreitet { $octal }

journal.cli.describe.pipeline.publish.move.spelling.contains.an.interior.nul = die Schreibweise des Verschiebens enthält im Inneren { $nul }

journal.cli.describe.pipeline.publish.parent = übergeordnetes Element der Veröffentlichung fehlt

journal.cli.describe.pipeline.publish.path = { $path }: { $detail }

journal.cli.describe.pipeline.publish.publication.path.is.not.valid.utf = der Veröffentlichungspfad ist nicht { $encoding }-gültig

journal.cli.describe.pipeline.publish.published.destination.identity.does.not.match.the.live.source = die Identität des veröffentlichten Ziels stimmt nicht mit dem aktuellen Ursprung überein

journal.cli.describe.pipeline.publish.published.destination.is.not.a.regular.file = das veröffentlichte Ziel ist keine reguläre Datei

journal.cli.describe.pipeline.publish.published.destination.name.is.absent = der Name des veröffentlichten Ziels ist nicht vorhanden

journal.cli.describe.pipeline.publish.source.and.destination.are.on.different.volumes = Ursprung und Ziel liegen auf verschiedenen Volumes

journal.cli.describe.pipeline.publish.source.and.destination.name.the.same.file = Ursprung und Ziel benennen dieselbe Datei

journal.cli.describe.pipeline.publish.source.has.no.normal.name = der Ursprung hat keinen normalen Namen

journal.cli.describe.pipeline.publish.source.identity.changed.before.publication = die Identität des Ursprungs hat sich vor der Veröffentlichung geändert

journal.cli.describe.pipeline.publish.source.identity.is.negative = die Identität des Ursprungs ist negativ

journal.cli.describe.pipeline.publish.source.name.changed.before.publication = der Name des Ursprungs hat sich vor der Veröffentlichung geändert

journal.cli.describe.pipeline.publish.source.name.is.absent.before.publication = der Name des Ursprungs ist vor der Veröffentlichung nicht vorhanden

journal.cli.describe.pipeline.publish.sync = der Abgleich des übergeordneten Elements der describe-Veröffentlichung ist nicht gelungen: { $error }

journal.cli.describe.pipeline.publish.uncertain = { $path }: { $operation }: { $detail }

journal.cli.describe.pipeline.reappear = das describe-Einzelbild { $frame } ist nicht mit demselben Zeitstempel wieder erschienen

journal.cli.describe.pipeline.record.parent.changed = das übergeordnete Element des Bildschirmtexts hat sich geändert

journal.cli.describe.pipeline.record.parent.plain = das übergeordnete Element des Bildschirmtexts ist kein einfaches Verzeichnis

journal.cli.describe.pipeline.reopen = das Video konnte von describe nicht erneut geöffnet werden, um die Nutzdaten der Einzelbilder zu materialisieren

journal.cli.describe.pipeline.resize = das Kategorisierungs-Einzelbild konnte nicht in der Größe geändert werden

journal.cli.describe.pipeline.response.id = die Sitzungsantwort hat die ID ausgelassen

journal.cli.describe.pipeline.response.unmatched = unkorrelierte Antwort

journal.cli.describe.pipeline.retry.binding = die Bindung des { $ocr }-Wiederholungs-Einzelbilds hat sich geändert

journal.cli.describe.pipeline.retry.evidence.missing = Wiederholungsnachweis fehlt

journal.cli.describe.pipeline.retry.handler = der native Fehlerhandler hat sich geändert

journal.cli.describe.pipeline.retry.id = die native Wiederholungsidentität ist nicht { $encoding }

journal.cli.describe.pipeline.retry.incomplete = die { $ocr }-Wiederholung hat das letzte qualifizierte Einzelbild nicht erreicht

journal.cli.describe.pipeline.retry.input.changed = die Wiederholungseingabe hat sich beim Lesen geändert

journal.cli.describe.pipeline.retry.input.regular = die Wiederholungseingabe ist keine reguläre Datei

journal.cli.describe.pipeline.retry.io.allocate = die Bereitstellungsdatei konnte nicht zugewiesen werden

journal.cli.describe.pipeline.retry.io.dest.namespace = der Zielnamensraum hat sich vor der Veröffentlichung geändert

journal.cli.describe.pipeline.retry.io.file.kind = der Eintrag ist keine reguläre Datei

journal.cli.describe.pipeline.retry.io.file.reparse = der Eintrag ist ein Reparsepunkt

journal.cli.describe.pipeline.retry.io.fsync = fsync des übergeordneten Verzeichnisses ist für { $path } beeinträchtigt: { $error }

journal.cli.describe.pipeline.retry.io.parent.identity = die gebundene Identität des übergeordneten Elements hat sich vor der Veröffentlichung geändert

journal.cli.describe.pipeline.retry.io.parent.kind = das übergeordnete Element ist kein echtes Verzeichnis

journal.cli.describe.pipeline.retry.io.parent.rename = der Pfadname des übergeordneten Elements hat sich vor der Veröffentlichung geändert

journal.cli.describe.pipeline.retry.io.parent.reparse = das übergeordnete Element ist ein Reparsepunkt

journal.cli.describe.pipeline.retry.io.prepare = die Identität der Bereitstellungsdatei hat sich beim Vorbereiten geändert

journal.cli.describe.pipeline.retry.io.stage.identity = die Identität der Bereitstellungsdatei hat sich vor der Veröffentlichung geändert

journal.cli.describe.pipeline.retry.io.stage.namespace = der Namensraum der Bereitstellungsdatei hat sich vor der Veröffentlichung geändert

journal.cli.describe.pipeline.retry.io.tmp = eine eindeutige temporäre Datei konnte nicht zugewiesen werden

journal.cli.describe.pipeline.retry.journal = das Journal der nativen Wiederholung stimmt nicht überein

journal.cli.describe.pipeline.retry.open = das Video für die { $ocr }-Wiederholung konnte nicht erneut geöffnet werden

journal.cli.describe.pipeline.retry.permission.changed = die Berechtigung der nativen Wiederholung hat sich geändert

journal.cli.describe.pipeline.retry.permission.missing = die Berechtigung der nativen Wiederholung fehlt

journal.cli.describe.pipeline.root.canonicalize.journal.root = Journal-Wurzel kanonisieren: { $detail }

journal.cli.describe.pipeline.root.cloud.files.sync.root.status.could.not.be.verified = der Status der { $cloud_files }-Abgleichwurzel konnte nicht überprüft werden

journal.cli.describe.pipeline.root.filesystem.is.not.ntfs.or.refs = das Dateisystem ist nicht { $ntfs } oder { $refs }

journal.cli.describe.pipeline.root.filesystem.type.could.not.be.verified = der Dateisystemtyp konnte nicht überprüft werden

journal.cli.describe.pipeline.root.journal.root.contains.a.reparse.point = die Journal-Wurzel enthält einen Reparsepunkt

journal.cli.describe.pipeline.root.journal.root.contains.an.interior.nul = die Journal-Wurzel enthält im Inneren { $nul }

journal.cli.describe.pipeline.root.journal.root.has.an.invalid.portable.name = die Journal-Wurzel hat einen ungültigen portablen Namen

journal.cli.describe.pipeline.root.journal.root.is.a.cloud.files.sync.root = die Journal-Wurzel ist eine { $cloud_files }-Abgleichwurzel

journal.cli.describe.pipeline.root.journal.root.is.a.reparse.point = die Journal-Wurzel ist ein Reparsepunkt

journal.cli.describe.pipeline.root.journal.root.is.on.a.non.local.volume = die Journal-Wurzel liegt auf einem nicht lokalen Volume

journal.cli.describe.pipeline.root.journal.root.is.on.a.ram.disk = die Journal-Wurzel liegt auf einer { $ram }-Disk

journal.cli.describe.pipeline.root.journal.root.is.on.removable.or.optical.media = die Journal-Wurzel liegt auf entfernbaren Speichermedien oder optischen Medien

journal.cli.describe.pipeline.root.journal.root.must.be.lexically.canonical = die Journal-Wurzel muss lexikalisch kanonisch sein

journal.cli.describe.pipeline.root.journal.root.must.use.a.fully.qualified.drive.path = die Journal-Wurzel muss einen vollständig qualifizierten Laufwerkspfad verwenden

journal.cli.describe.pipeline.root.journal.root.volume.type.is.unknown = der Volumetyp der Journal-Wurzel ist unbekannt

journal.cli.describe.pipeline.root.open.filesystem.root = Dateisystemwurzel öffnen: { $detail }

journal.cli.describe.pipeline.root.open.journal.directory = Journal-Verzeichnis öffnen: { $detail }

journal.cli.describe.pipeline.root.open.journal.root = Journal-Wurzel öffnen: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.ancestor = Vorgänger der Journal-Wurzel öffnen: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.namespace.watch = Namespace-Überwachung der Journal-Wurzel öffnen: { $detail }

journal.cli.describe.pipeline.root.open.verified.drive.root = überprüfte Laufwerkswurzel öffnen: { $detail }

journal.cli.describe.pipeline.root.open.verified.journal.root = überprüfte Journal-Wurzel öffnen: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.ancestor.attributes = Attribute des Vorgängers der Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.attributes = Attribute der Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.drive.type = Laufwerkstyp der Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.identity = Identität der Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.attributes = Attribute der Namespace-Überwachung der Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.identity = Identität der Namespace-Überwachung der Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.attributes = Attribute der behaltenen Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.identity = Identität der behaltenen Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.attributes = Attribute der überprüften Laufwerkswurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.identity = Identität der überprüften Laufwerkswurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.attributes = Attribute der überprüften Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.identity = Identität der überprüften Journal-Wurzel abfragen: { $detail }

journal.cli.describe.pipeline.root.read.source.file.identity = Identität der Ursprungsdatei lesen: { $detail }

journal.cli.describe.pipeline.root.restat.acquired.journal.root = restat der erlangten Journal-Wurzel: { $detail }

journal.cli.describe.pipeline.root.stat.acquired.journal.root = stat der erlangten Journal-Wurzel: { $detail }

journal.cli.describe.pipeline.root.stat.canonical.root.component = stat der kanonischen Wurzelkomponente: { $detail }

journal.cli.describe.pipeline.root.stat.opened.canonical.root.component = stat der geöffneten kanonischen Wurzelkomponente: { $detail }

journal.cli.describe.pipeline.root.stat.opened.filesystem.root = stat der geöffneten Dateisystemwurzel: { $detail }

journal.cli.describe.pipeline.row.frame = die describe-Zeile hat { $frame_key } ausgelassen

journal.cli.describe.pipeline.source.bytes = { $ocr }-Ursprungsbytes geändert

journal.cli.describe.pipeline.source.changed = { $ocr }-Ursprung vor der Veröffentlichung geändert

journal.cli.describe.pipeline.source.identity = { $ocr }-Ursprungsidentität geändert

journal.cli.describe.pipeline.source.open = { $ocr }-Ursprung beim Öffnen geändert

journal.cli.describe.pipeline.source.plain = { $ocr }-Ursprung ist keine einfache Datei

journal.cli.describe.pipeline.temp = die temporäre describe-Datei konnte nicht angelegt werden

journal.cli.describe.pipeline.timestamp = der describe-Frame { $frame } hat den Zeitstempel ausgelassen

journal.cli.describe.pipeline.windows.cleanup = { $detail }; die Stufe { $stage } konnte nicht entfernt werden: { $error }

journal.cli.describe.pipeline.windows.name.alternate.data.stream = der Name enthält einen Doppelpunkt

journal.cli.describe.pipeline.windows.name.control = der Name enthält ein Steuerzeichen

journal.cli.describe.pipeline.windows.name.dot.component = '.' und '..' sind keine gültigen Namen

journal.cli.describe.pipeline.windows.name.empty = der Name ist leer

journal.cli.describe.pipeline.windows.name.forbidden.character = der Name enthält ein Zeichen, das { $windows } nicht erlaubt

journal.cli.describe.pipeline.windows.name.reserved.device = der Name ist von { $windows } reserviert

journal.cli.describe.pipeline.windows.name.root.or.prefix = der Name muss eine einzelne relative Komponente sein

journal.cli.describe.pipeline.windows.name.separator = der Name enthält einen Pfadtrenner

journal.cli.describe.pipeline.windows.name.too.long = der Name ist länger als { $length_limit } { $encoding }-Bytes

journal.cli.describe.pipeline.windows.name.trailing.dot.or.space = der Name endet mit einem Punkt oder Leerzeichen

journal.cli.describe.pipeline.windows.path.already.exists = der Vorgänger '{ $component }' des Veröffentlichungspfads erschien während Nur-Erstellen

journal.cli.describe.pipeline.windows.path.classify.install.source = Installationsursprung klassifizieren: { $detail }

journal.cli.describe.pipeline.windows.path.component = die Komponente des Veröffentlichungspfads ist nicht portabel: { $detail }

journal.cli.describe.pipeline.windows.path.create.publication.path.ancestor = Vorgänger des Veröffentlichungspfads erstellen: { $detail }

journal.cli.describe.pipeline.windows.path.dot.component = der Veröffentlichungspfad enthält '.' oder '..'

journal.cli.describe.pipeline.windows.path.drive.relative = der Veröffentlichungspfad ist laufwerksrelativ

journal.cli.describe.pipeline.windows.path.empty = der Veröffentlichungspfad ist leer

journal.cli.describe.pipeline.windows.path.identity.changed = die Identität des Veröffentlichungspfads hat sich auf der behaltenen Ebene { $level } geändert

journal.cli.describe.pipeline.windows.path.identity.mismatch = die Identität des Vorgängers '{ $component }' des Veröffentlichungspfads hat sich zwischen Erstellen und erneutem Öffnen geändert

journal.cli.describe.pipeline.windows.path.interior.nul = der Veröffentlichungspfad enthält im Inneren { $nul }

journal.cli.describe.pipeline.windows.path.missing = die Komponente '{ $component }' des Veröffentlichungspfads existiert nicht

journal.cli.describe.pipeline.windows.path.move.spelling.diverged = die Verschiebe-Schreibweise des Veröffentlichungspfads nennt nicht mehr das letzte übergeordnete Verzeichnis

journal.cli.describe.pipeline.windows.path.not.directory = der Vorgänger '{ $component }' des Veröffentlichungspfads ist kein Verzeichnis

journal.cli.describe.pipeline.windows.path.not.regular.file = das Blatt '{ $component }' des Veröffentlichungspfads ist keine reguläre Datei

journal.cli.describe.pipeline.windows.path.open.install.source = Installationsursprung öffnen: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.ancestor = Vorgänger des Veröffentlichungspfads öffnen: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.anchor = Anker des Veröffentlichungspfads öffnen: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.move.spelling = Verschiebe-Schreibweise des Veröffentlichungspfads öffnen: { $detail }

journal.cli.describe.pipeline.windows.path.path.too.long = der Veröffentlichungspfad überschreitet die Grenze der erweiterten Länge

journal.cli.describe.pipeline.windows.path.query.install.path.attributes = Attribute des Installationspfads abfragen: { $detail }

journal.cli.describe.pipeline.windows.path.query.install.source.identity = Identität des Installationsursprungs abfragen: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.attributes = Attribute des Veröffentlichungspfads abfragen: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.identity = Identität des Veröffentlichungspfads abfragen: { $detail }

journal.cli.describe.pipeline.windows.path.reopen.publication.path.ancestor = Vorgänger des Veröffentlichungspfads erneut öffnen: { $detail }

journal.cli.describe.pipeline.windows.path.reparse.point = der Vorgänger '{ $component }' des Veröffentlichungspfads ist ein Reparsepunkt

journal.cli.describe.pipeline.windows.path.resolve.publication.path.current.directory = aktuelles Verzeichnis des Veröffentlichungspfads auflösen: { $detail }

journal.cli.describe.pipeline.windows.path.revalidate.install.source.after.classification = Installationsursprung nach der Klassifizierung erneut validieren: { $detail }

journal.cli.describe.pipeline.windows.path.root.only = der Veröffentlichungspfad ist eine Laufwerkswurzel ohne Ziel

journal.cli.describe.pipeline.windows.path.rooted.without.drive = der Veröffentlichungspfad ist ohne Laufwerk verwurzelt

journal.cli.describe.pipeline.windows.path.unc.or.device.prefix = der Veröffentlichungspfad hat ein { $unc }- oder Gerätepräfix

journal.cli.describe.pipeline.windows.path.unstable = der Veröffentlichungspfad hat kein { $manager }-Volume { $guid }: { $detail }

journal.cli.describe.pipeline.withdraw = die Rücknahme des Bildschirms { $ocr } konnte nicht vorbereitet werden: { $detail }

journal.cli.describe.settings.finite = { $setting } muss eine endliche Zahl sein

journal.cli.describe.settings.importance = { $setting } muss { $ignore }, { $low }, { $normal } oder { $high } sein

journal.cli.describe.settings.object = { $setting } muss ein Objekt sein

journal.cli.describe.settings.string = { $setting } muss eine Zeichenkette sein

journal.cli.describe.settings.strings = { $setting } muss ein Array von Zeichenketten sein

journal.cli.describe.settings.uint = { $setting } muss eine vorzeichenlose 32-Bit-Ganzzahl sein

journal.cli.describe.stored.error.empty = Leere Markdown-Antwort für { $category }

journal.cli.describe.stored.error.incomplete = Die Extraktion wurde nie abgeschlossen

journal.cli.describe.stored.error.invalid = Ungültige { $format }-Antwort

journal.cli.describe.stored.error.invalid.category = Ungültige { $format }-Antwort für { $category }: { $error }

journal.cli.describe.stored.error.resize = der Extraktionsframe konnte nicht in der Größe geändert werden

journal.cli.describe.stored.error.schema = Die Schemavalidierung für { $category } ist fehlgeschlagen

journal.cli.describe.stored.error.schema.detail = Die Schemavalidierung für { $category } ist fehlgeschlagen: { $errors }

journal.cli.describe.stored.error.truncated = Abgeschnittene Markdown-Antwort für { $category }

journal.cli.describe.stored.note.bounded = [{ $mark }: Extraktionsausgabe vor dem Schreiben ins Journal begrenzt - entartete Länge bereinigt/abgeschnitten]

journal.cli.describe.stored.note.ocr = Text, der von deinen Bildschirmen erkannt wurde. er kann Erkennungsfehler enthalten. das sagt, was sichtbar war, nicht was du gelesen, getippt oder getan hast. das Maskieren von Passwörtern und anderen Geheimnissen geschieht nach bestem Bemühen; deine Schwärzungsregeln gelten nicht.

journal.cli.describe.stored.ocr.heading = { $clock } · { $category }

journal.cli.describe.usage.argument.unknown = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: unbekanntes Argument: { $argument }

journal.cli.describe.usage.backfill.arguments = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: Gesichts-Auffüllen erfordert { $arguments }

journal.cli.describe.usage.backfill.budget = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: das Budget des Gesichts-Auffüllens liegt außerhalb des erlaubten Bereichs

journal.cli.describe.usage.backfill.desktop = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: Gesichts-Auffüllen ist nur für Desktop-Journale verfügbar

journal.cli.describe.usage.describe.repeated = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $describe_flag } wurde mehr als einmal angegeben

journal.cli.describe.usage.file.extra = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: es darf nur ein Videopfad angegeben werden

journal.cli.describe.usage.file.required = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: folgende Argumente sind erforderlich: { $file }

journal.cli.describe.usage.frames.repeated = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $frames_only_flag } wurde mehr als einmal angegeben

journal.cli.describe.usage.help = Verwendung: { $syntax }

journal.cli.describe.usage.help.extra = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $help_flag } akzeptiert keine anderen Argumente

journal.cli.describe.usage.jobs.positive = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $jobs_flag } erfordert eine positive Ganzzahl

journal.cli.describe.usage.jobs.repeated = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $jobs_flag } wurde mehr als einmal angegeben

journal.cli.describe.usage.journal.path = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $journal_flag } erfordert einen Pfad

journal.cli.describe.usage.journal.repeated = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $journal_flag } wurde mehr als einmal angegeben

journal.cli.describe.usage.mode.required = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: genau eines von { $frames_only_flag } oder { $describe_flag } ist erforderlich

journal.cli.describe.usage.redo.mode = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $redo_flag } erfordert { $describe_flag }

journal.cli.describe.usage.redo.repeated = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $redo_flag } wurde mehr als einmal angegeben

journal.cli.describe.usage.registry.extra = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $category_registry_flag } akzeptiert keine anderen Argumente

journal.cli.describe.usage.version.extra = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: { $version_flag } akzeptiert keine anderen Argumente

journal.cli.grab.alternatives.days = Verfügbare Tage (die nächsten 5):

journal.cli.grab.alternatives.screens = Verfügbare Bildschirme in { $day }/{ $stream }/{ $segment }:

journal.cli.grab.alternatives.segments = Verfügbare Segmente in { $day }/{ $stream }:

journal.cli.grab.alternatives.streams = Verfügbare Datenströme in { $day }:

journal.cli.grab.error.captured.not.analyzed = Bildschirm { $screen } in { $segment } ist erfasst, aber nicht analysiert

journal.cli.grab.error.day.not.found = Tag { $day } nicht gefunden

journal.cli.grab.error.decode.failed.ids = die Frame-IDs konnten nicht dekodiert werden: { $ids }

journal.cli.grab.error.decode.invalid.rgb = das Video konnte nicht dekodiert werden: ungültiger RGB-Frame

journal.cli.grab.error.decode.no.video = das Video konnte nicht dekodiert werden: kein Videodatenstrom

journal.cli.grab.error.decode.video = das Video konnte nicht dekodiert werden: { $error }

journal.cli.grab.error.decoder.init = der Videodecoder konnte nicht initialisiert werden: { $error }

journal.cli.grab.error.force.requires.out = { $force_flag } erfordert { $out_flag }

journal.cli.grab.error.frame.id.positive = Frame-IDs müssen positive Ganzzahlen sein: erhalten '{ $token }'

journal.cli.grab.error.frame.id.unique = Frame-IDs müssen eindeutig sein: { $frame_id }

journal.cli.grab.error.frame.not.found = Frame-ID { $frame_id } nicht gefunden in { $screen } für { $segment }

journal.cli.grab.error.invalid.frame.id = ungültige Frame-ID

journal.cli.grab.error.invalid.level4 = ungültige Nutzdaten der Ebene 4

journal.cli.grab.error.invalid.level5a = ungültige Nutzdaten der Ebene 5a

journal.cli.grab.error.invalid.render.payload = ungültige Darstellungs-Nutzdaten

journal.cli.grab.error.invalid.segment.key = Segment { $segment } ist kein gültiger { $format }-Schlüssel

journal.cli.grab.error.io = der Zugriff auf { $path } ist nicht gelungen: { $error }

journal.cli.grab.error.legacy.schema = die Bildschirmdatei verwendet das Schema vor frame_id; die Frame-Auswahl ist nicht verfügbar

journal.cli.grab.error.multiple.ids.require.out = mehrere Frame-IDs erfordern { $out_flag }

journal.cli.grab.error.out.exists = der Ausgabepfad existiert (verwende { $force_flag }): { $paths }

journal.cli.grab.error.out.extension = { $out_flag } muss auf { $extensions } enden

journal.cli.grab.error.out.filename = der Ausgabepfad hat keinen gültigen Dateinamen

journal.cli.grab.error.out.requires.five.tokens = { $out_flag } erfordert day stream segment screen und frame-id

journal.cli.grab.error.save.image = das Bild konnte nicht gespeichert werden: { $error }

journal.cli.grab.error.screen.not.found = Bildschirm { $screen } nicht gefunden in { $day }/{ $stream }/{ $segment }

journal.cli.grab.error.segment.not.found = Segment { $segment } nicht gefunden in { $day }/{ $stream }

journal.cli.grab.error.stream.not.found = Datenstrom { $stream } nicht gefunden in { $day }

journal.cli.grab.error.timestamp.finite = der Zeitstempel muss endlich sein

journal.cli.grab.error.timestamp.invalid = ungültiger Zeitstempel

journal.cli.grab.error.timestamp.range = der Zeitstempel liegt außerhalb des Bereichs

journal.cli.grab.error.too.many.tokens = grab akzeptiert höchstens 5 positionelle Token: day stream segment screen frame-id

journal.cli.grab.error.unsupported.level = nicht unterstützte Ausgabeebene

journal.cli.grab.error.unsupported.level.named = nicht unterstützte Ausgabeebene: { $level }

journal.cli.grab.error.video.missing = das Rohvideo für Bildschirm { $screen } in { $segment } wurde nicht gefunden

journal.cli.grab.error.video.missing.with.reason = { $reason }; der Zugriff nur auf Metadaten bleibt über: { $command }

journal.cli.grab.frame.note.error = Fehler: { $text }

journal.cli.grab.header.abs.time = abs_time

journal.cli.grab.header.connector = Anschluss

journal.cli.grab.header.day = Tag

journal.cli.grab.header.end = Ende

journal.cli.grab.header.frame.id = frame_id

journal.cli.grab.header.frames.analyzed = frames_analyzed

journal.cli.grab.header.notes = Notizen

journal.cli.grab.header.position = Position

journal.cli.grab.header.primary = primär

journal.cli.grab.header.screen = Bildschirm

journal.cli.grab.header.screens = Bildschirme

journal.cli.grab.header.segment = Segment

journal.cli.grab.header.segments = Segmente

journal.cli.grab.header.start = Start

journal.cli.grab.header.status = Status

journal.cli.grab.header.stream = Datenstrom

journal.cli.grab.header.streams = Datenströme

journal.cli.grab.header.timestamp = Zeitstempel

journal.cli.grab.level4.empty = Keine qualifizierten Frames in der Analyse dieses Bildschirms.

journal.cli.grab.level4.footer = Ansehen: { $inspect_command }{ $line_break }Eins speichern: { $save_one_command }{ $line_break }Mehrere speichern: { $save_many_command }{ $line_break }{ $line_break }Wie die Extraktion funktioniert:{ $line_break }  Das Dekodieren geht das Video linear ab Frame 0 durch — Spulen ist unsicher bei der{ $line_break }  Aufnahmerate von 1 Hz. Für die Kosten gilt die höchste angeforderte frame_id, nicht{ $line_break }  die Anzahl. Die IDs 7,12,23 anzufordern kostet dasselbe wie 23 allein anzufragen.{ $line_break }  Bevorzuge den Stapelmodus, wenn du mehr als ein Frame von demselben Bildschirm willst.

journal.cli.grab.level4.legacy = 0 Frames analysiert: die Datei verwendet das Schema vor frame_id

journal.cli.grab.level4.missing.video = Speichermodus nicht verfügbar: { $reason }.{ $line_break }Die Frame-Metadaten oben sind weiterhin lesbar.{ $line_break }{ $line_break }Ansehen: { $command }

journal.cli.grab.level5a.footer = Speichern: { $save_command }{ $line_break }Stapel: { $batch_command }

journal.cli.grab.level5a.frame = Frame: { $value }

journal.cli.grab.level5a.jsonl = JSONL: { $value }

journal.cli.grab.level5a.notes = Notizen: { $value }

journal.cli.grab.level5a.screen = Bildschirm: { $value }

journal.cli.grab.level5a.time = Zeit: { $value }

journal.cli.grab.level5a.video = Video: { $value }

journal.cli.grab.missing.reason.offload = du hast das Originalvideo gelöscht, nachdem deine Sicherung es kopiert hat

journal.cli.grab.missing.reason.owner = du hast das Originalvideo gelöscht

journal.cli.grab.missing.reason.policy = du hast das Originalvideo gelöscht, nachdem deine Aufbewahrungseinstellungen es markiert haben

journal.cli.grab.missing.reason.trash = das Originalvideo liegt im Papierkorb, wo du es in den Einstellungen wiederherstellen kannst

journal.cli.grab.missing.reason.unknown = das Originalvideo ist nicht mehr in deinem Journal

journal.cli.grab.next = Weiter: { $command }

journal.cli.grab.saved = gespeichert { $path }

journal.cli.grab.status.analyzed = analysiert

journal.cli.grab.status.captured = erfasst, aber nicht analysiert

journal.cli.grab.status.missing.offload = analysiert; du hast das Originalvideo gelöscht, nachdem deine Sicherung es kopiert hat

journal.cli.grab.status.missing.owner = analysiert; du hast das Originalvideo gelöscht

journal.cli.grab.status.missing.policy = analysiert; du hast das Originalvideo gelöscht, nachdem deine Aufbewahrungseinstellungen es markiert haben

journal.cli.grab.status.missing.trash = analysiert; das Originalvideo liegt im Papierkorb

journal.cli.grab.status.missing.unknown = analysiert; das Originalvideo ist nicht mehr in deinem Journal

journal.cli.maintenance.backup.erase = endgültiges Löschen der Sicherung: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending }

journal.cli.maintenance.backup.erase.error.detail = endgültiges Löschen der Sicherung: Fehler { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.erase.reason = endgültiges Löschen der Sicherung: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending } { $reason_field }{ $reason }

journal.cli.maintenance.backup.error = Sicherung: Fehler { $reason_field }{ $reason }

journal.cli.maintenance.backup.error.detail = Sicherung: Fehler { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.ok = Sicherung: ok { $field }{ $snapshot }

journal.cli.maintenance.backup.paused = Sicherung: übersprungen. die Sicherung ist pausiert, also ist nichts gelaufen. führe `{ $resume }` aus, um sie wieder zu starten.

journal.cli.maintenance.backup.prune.error = Ausdünnen der Sicherung: Fehler { $reason_field }{ $reason }

journal.cli.maintenance.backup.prune.error.detail = Ausdünnen der Sicherung: Fehler { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.prune.ok = Ausdünnen der Sicherung: ok

journal.cli.maintenance.backup.prune.paused = Ausdünnen der Sicherung: übersprungen. die Sicherung ist pausiert, also ist nichts gelaufen. führe `{ $resume }` aus, um sie wieder zu starten.

journal.cli.maintenance.backup.prune.skipped = Ausdünnen der Sicherung: übersprungen

journal.cli.maintenance.backup.skipped = Sicherung: übersprungen

journal.cli.maintenance.backup.superseded = Sicherung: von einem anderen Gerät beansprucht; lokale Einstellungen wurden entfernt. führe `{ $enable }` aus, um hier eine neue Sicherung einzurichten.

journal.cli.maintenance.backup.unavailable = die Wartungsdienste der Sicherung sind nicht verfügbar.

journal.cli.maintenance.backup.usage.offload = Verwendung: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: Fehler: unbekannte Argumente: { $args }

journal.cli.maintenance.backup.usage.routine = Verwendung: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: Fehler: unbekannte Argumente: { $args }

journal.cli.maintenance.backup.verify.error = Überprüfen der Sicherung: Fehler { $reason_field }{ $reason }

journal.cli.maintenance.backup.verify.error.detail = Überprüfen der Sicherung: Fehler { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.verify.ok = Überprüfen der Sicherung: ok { $field }{ $subset }

journal.cli.maintenance.backup.verify.paused = Überprüfen der Sicherung: übersprungen. die Sicherung ist pausiert, also ist nichts gelaufen. führe `{ $resume }` aus, um sie wieder zu starten.

journal.cli.maintenance.backup.verify.skipped = Überprüfen der Sicherung: übersprungen

journal.cli.maintenance.description.backup.erase = auch aus deinen Sicherungen entfernen, was du endgültig gelöscht hast, wenn du das gewählt hast.

journal.cli.maintenance.description.backup.offload = überprüfte Rohmedien nach der Sicherung auslagern.

journal.cli.maintenance.description.backup.prune = die Aufbewahrungsrichtlinie der verschlüsselten Sicherung anwenden.

journal.cli.maintenance.description.backup.run = verschlüsselte Sicherung ausführen.

journal.cli.maintenance.description.backup.verify = das Rücklesen der verschlüsselten Sicherung überprüfen.

journal.cli.maintenance.description.faces.backfill = Profilbildvorschläge aus Besprechungen auffrischen, einschließlich vergangener Besprechungen.

journal.cli.maintenance.description.health.mark.raw = originale Medien auflisten, die zur Entfernung bereit sind.

journal.cli.maintenance.description.health.prune.logs = alte Betriebsprotokolle ausdünnen.

journal.cli.maintenance.description.retention.owner.voice.originals = das Löschen des originalen Audios neuerer Aufnahmen abschließen, die mit "andere Stimmen auslassen" an verarbeitet wurden.

journal.cli.maintenance.description.speakers.backfill.labels = die Sprecher benennen, ohne das Modell, in Audio, das noch keine Sprecherbeschriftungen hat.

journal.cli.maintenance.description.speakers.backfill.pool = die Stimmenliste aus bereits transkribiertem Audio aufholen, dann nach deiner Stimme suchen.

journal.cli.maintenance.description.speakers.candidate.pair.suggestions = Finde dichte Sprecherkandidatenpaare für Vorschläge.

journal.cli.maintenance.description.speakers.consolidate.pool = Führe dichte Sprecherkandidaten zusammen.

journal.cli.maintenance.description.speakers.discover.voices = Frisch den Zwischenspeicher der Erkennung wiederkehrender Stimmen auf.

journal.cli.maintenance.description.speakers.name.variants = Finde Namensvarianten von Sprechern für Vorschläge.

journal.cli.maintenance.description.trash.erase = Papierkorbschritte abschließen, die ein Stopp unterbrochen hat, und endgültig löschen, was seine Tage abgewartet hat.

journal.cli.maintenance.face.generation = die Generierung des Gesichts-Auffüllens wurde nicht zugelassen

journal.cli.maintenance.face.log.cache = der Scan-Zwischenspeicher des Gesichts-Auffüllens wurde nicht gespeichert: { $detail }

journal.cli.maintenance.face.log.deferred = Wiederherstellung der Gesichtsbeteiligung zurückgestellt: { $detail }

journal.cli.maintenance.face.log.failed = das Gesichts-Auffüllen ist fehlgeschlagen: { $detail }

journal.cli.maintenance.face.priority = Priorität des Gesichtsverlaufs nicht verfügbar: { $error }

journal.cli.maintenance.face.reconciliation = Gesichtsabstimmung: { $error }

journal.cli.maintenance.health.config.days = { $field } muss eine positive Ganzzahl sein

journal.cli.maintenance.health.config.enabled = { $field } muss ein Wahrheitswert sein

journal.cli.maintenance.health.keep = { $routine }: deine Aufbewahrungseinstellungen behalten alle originalen Medien.

journal.cli.maintenance.health.mark.entry = { $indent }{ $id }: { $reason }

journal.cli.maintenance.health.mark.refused = { $routine }: einige Elemente konnten nicht aufgelistet werden:

journal.cli.maintenance.health.mark.summary = { $routine }: neue Elemente: { $new }.{ $line_break }  Gesamtstand: { $total }.

journal.cli.maintenance.health.mark.unavailable = { $routine }: die Liste konnte nicht erstellt werden: { $error }

journal.cli.maintenance.health.prune.args.integer = Verwendung: { $command } { $syntax }{ $line_break }{ $command }: Fehler: Argument { $flag }: ungültiger Ganzzahlwert: '{ $value }'

journal.cli.maintenance.health.prune.args.missing = Verwendung: { $command } { $syntax }{ $line_break }{ $command }: Fehler: Argument { $flag }: genau ein Argument erwartet

journal.cli.maintenance.health.prune.args.positive = { $routine }: { $flag } muss eine positive Ganzzahl sein

journal.cli.maintenance.health.prune.args.unexpected = Verwendung: { $command } { $syntax }{ $line_break }{ $command }: Fehler: unbekannte Argumente: { $value }

journal.cli.maintenance.health.prune.class = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } Datei, { $dirs } Zwischenspeicherverzeichnis, { $bytes }
       *[other] { $indent }{ $class }: { $files } Datei, { $dirs } Zwischenspeicherverzeichnisse, { $bytes }
    }
   *[other] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } Dateien, { $dirs } Zwischenspeicherverzeichnis, { $bytes }
       *[other] { $indent }{ $class }: { $files } Dateien, { $dirs } Zwischenspeicherverzeichnisse, { $bytes }
    }
}

journal.cli.maintenance.health.prune.disabled = { $routine }: deaktiviert

journal.cli.maintenance.health.prune.halted = { $routine }: { $message }

journal.cli.maintenance.health.prune.missing = das Journal-Verzeichnis ist nicht verfügbar

journal.cli.maintenance.health.prune.preview = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: würde { $files } Betriebsprotokoll-Datei, { $dirs } Zwischenspeicherverzeichnis, { $bytes } ausdünnen { $cutoff_field }{ $cutoff }
       *[other] { $routine }: würde { $files } Betriebsprotokoll-Datei, { $dirs } Zwischenspeicherverzeichnisse, { $bytes } ausdünnen { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: würde { $files } Betriebsprotokoll-Dateien, { $dirs } Zwischenspeicherverzeichnis, { $bytes } ausdünnen { $cutoff_field }{ $cutoff }
       *[other] { $routine }: würde { $files } Betriebsprotokoll-Dateien, { $dirs } Zwischenspeicherverzeichnisse, { $bytes } ausdünnen { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.queue = ein Protokoll wurde entfernt, aber seine Suchaktualisierung konnte nicht in die Warteschlange gestellt werden: { $error }

journal.cli.maintenance.health.prune.refused = { $routine }: einige Protokolle konnten nicht ausgedünnt werden: { $summary }

journal.cli.maintenance.health.prune.refused.unnamed = { $routine }: einige Protokolle konnten nicht ausgedünnt werden: das Aufbewahrungswerkzeug hat abgelehnt, ohne einen Eintrag zu nennen.

journal.cli.maintenance.health.prune.result = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: { $files } Betriebsprotokoll-Datei ausgedünnt, { $dirs } Zwischenspeicherverzeichnis, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: { $files } Betriebsprotokoll-Datei ausgedünnt, { $dirs } Zwischenspeicherverzeichnisse, { $bytes } { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: { $files } Betriebsprotokoll-Dateien ausgedünnt, { $dirs } Zwischenspeicherverzeichnis, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: { $files } Betriebsprotokoll-Dateien ausgedünnt, { $dirs } Zwischenspeicherverzeichnisse, { $bytes } { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.unavailable = { $routine }: Protokolle konnten nicht ausgedünnt werden: { $error }

journal.cli.maintenance.health.prune.undateable = { $indent }Fehler: das Aufbewahrungsdatum des Eintrags konnte nicht bestimmt werden: das Aufbewahrungsdatum des Eintrags konnte nicht bestimmt werden

journal.cli.maintenance.health.prune.unreadable = { $indent }Fehler: { $detail }: { $detail }

journal.cli.maintenance.health.unavailable = die Wartungsdienste für den Zustand sind nicht verfügbar.

journal.cli.maintenance.health.usage = Verwendung: { $command } { $id } { $syntax }{ $line_break }{ $command } { $id }: Fehler: unbekannte Argumente: { $args }

journal.cli.maintenance.help.list = Verwendung: { $syntax }

journal.cli.maintenance.help.root = Verwendung: { $syntax }

journal.cli.maintenance.help.run = Verwendung: { $syntax }

journal.cli.maintenance.help.sync = Verwendung: { $syntax }

journal.cli.maintenance.list.empty = Keine Wartungsroutinen gefunden.

journal.cli.maintenance.list.heading.description = BESCHREIBUNG

journal.cli.maintenance.list.heading.every = ALLE

journal.cli.maintenance.list.heading.id = ID

journal.cli.maintenance.list.heading.runtime = MAX. LAUFZEIT

journal.cli.maintenance.list.heading.status = STATUS

journal.cli.maintenance.list.status.divergent = abweichend

journal.cli.maintenance.list.status.missing = fehlt

journal.cli.maintenance.list.status.nothere = n. z.

journal.cli.maintenance.list.status.synced = abgeglichen

journal.cli.maintenance.missing.root = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: folgende Argumente sind erforderlich: { $argument }

journal.cli.maintenance.missing.run = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: folgende Argumente sind erforderlich: { $argument }

journal.cli.maintenance.retention.import.owed = Bereinigung des importierten Audios unvollständig: { $error }

journal.cli.maintenance.retention.owed = Bereinigung des Originals der Eigentümerstimme unvollständig: { $error }

journal.cli.maintenance.retention.summary = { $routine }: gelöscht { $deleted }, ohne Datensatz gelöscht { $deleted_unrecorded }, bereits weg { $gone }, noch ausstehend { $owed }, für immer behalten { $refused }, nicht nachweisbar { $not_provable }, nichts ausstehend { $not_owed }, bei Beschäftigung übersprungen { $skipped }, Ereignisse unlesbar { $unknown }

journal.cli.maintenance.retention.usage = Verwendung: { $id } nimmt keine Argumente.

journal.cli.maintenance.routine.unimplemented = { $count_quantity ->
    [one] Wartungsroutine { $id } ist noch nicht implementiert ({ $count } weitergereichtes Argument).
   *[other] Wartungsroutine { $id } ist noch nicht implementiert ({ $count } weitergereichte Argumente).
}

journal.cli.maintenance.routine.unknown = Unbekannte Wartungsroutine: { $id }. Führ `{ $command }` aus, um die verfügbaren Routinen zu sehen.

journal.cli.maintenance.schedule.error = Fehler beim Lesen/Aktualisieren von { $path }: { $error } (Ursache: { $error })

journal.cli.maintenance.schedule.io = die E/A des Zeitplans ist nicht gelungen: { $detail }

journal.cli.maintenance.schedule.malformed = fehlerhafte Konfiguration der Zeitpläne unter { $path }

journal.cli.maintenance.schedule.metadata = unbekannte Metadatenschlüssel des Zeitplans: { $keys }

journal.cli.maintenance.schedule.shape = { $path }: die Konfiguration der Zeitpläne muss ein JSON-Objekt sein

journal.cli.maintenance.schedule.state.shape = der Zustand des Zeitplans unter { $path } muss ein JSON-Objekt sein

journal.cli.maintenance.speakers.discovery.helper = das Hilfsprogramm der Sprecherentdeckung ist bei { $stage } nicht gelungen: { $reason }

journal.cli.maintenance.speakers.discovery.input = die Sprecherentdeckung ist nicht gelungen: { $detail }

journal.cli.maintenance.speakers.error.owner.voice.look.failed = das Nachschlagen der Eigentümerstimme ist nicht gelungen: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.consolidation.failed = die Konsolidierung der Sprecherkandidaten ist nicht gelungen: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.pair.refresh.failed = die Auffrischung der Sprecher-Kandidatenpaare ist nicht gelungen: { $error }

journal.cli.maintenance.speakers.error.speaker.label.backfill.failed = das Auffüllen der Sprecherbeschriftung ist nicht gelungen: { $error }

journal.cli.maintenance.speakers.error.speaker.name.variant.refresh.failed = die Auffrischung der Sprechernamensvarianten ist nicht gelungen: { $error }

journal.cli.maintenance.speakers.not.admitted = die Generation der Sprecherentdeckung wurde beim Wartungseintritt nicht zugelassen

journal.cli.maintenance.speakers.usage.backfill.labels = Verwendung: { $command }

journal.cli.maintenance.speakers.usage.backfill.pool = Verwendung: { $command }

journal.cli.maintenance.speakers.usage.candidate.pair.suggestions = Verwendung: { $command }

journal.cli.maintenance.speakers.usage.consolidate.pool = Verwendung: { $command }

journal.cli.maintenance.speakers.usage.discover.voices = Verwendung: { $command }

journal.cli.maintenance.speakers.usage.name.variants = Verwendung: { $command }

journal.cli.maintenance.sync.added = hinzugefügt: { $count }: { $ids }

journal.cli.maintenance.sync.added.empty = hinzugefügt: 0

journal.cli.maintenance.sync.disabled = deaktiviert: { $count }: { $ids }

journal.cli.maintenance.sync.disabled.empty = deaktiviert: 0

journal.cli.maintenance.sync.divergent = abweichend: { $count }: { $ids }

journal.cli.maintenance.sync.divergent.empty = abweichend: 0

journal.cli.maintenance.sync.removed = entfernt: { $count }: { $ids }

journal.cli.maintenance.sync.removed.empty = entfernt: 0

journal.cli.maintenance.sync.synced = abgeglichen: { $count }: { $ids }

journal.cli.maintenance.sync.synced.empty = abgeglichen: 0

journal.cli.maintenance.sync.warning.disabled = WARNUNG: Zeitplan { $id } ist deaktiviert; deaktiviert beibehalten

journal.cli.maintenance.sync.warning.divergent = WARNUNG: Zeitplan { $id } ist abweichend; unverändert beibehalten

journal.cli.maintenance.trash.erased = Papierkorb: endgültig gelöscht { $erased }, abgeschlossen { $finished }, wartet { $waiting }, fehlgeschlagen { $failed }

journal.cli.maintenance.trash.error = Papierkorb: { $failure }

journal.cli.maintenance.trash.usage = Verwendung: { $id } nimmt keine Argumente.

journal.cli.maintenance.usage.face = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: unbekannte Argumente: unerwartete Argumente zum Gesichtsabgleich

journal.cli.maintenance.usage.unknown = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.offload.guidance.changed.retry = die Auslagerung wurde gestoppt, weil sich dein Journal geändert hat, während sie lief. von diesem Computer wurde nichts entfernt. führ sie erneut aus.

journal.cli.offload.guidance.changed.review = die Auslagerung wurde gestoppt, weil sich dein Journal geändert hat, während sie lief. von diesem Computer wurde nichts entfernt. behalte die Wiederherstellungsdateien der Auslagerung und sieh die aufgezeichneten Auswirkungen durch, bevor du es erneut versuchst.

journal.cli.offload.guidance.clock = die Uhr des Vorgangs konnte nicht bestätigt werden.

journal.cli.offload.guidance.journal = dein Journal konnte nicht bestätigt werden. behalte seine Wiederherstellungsdateien zur Durchsicht.

journal.cli.offload.guidance.platform = die Ausführung der Auslagerung ist auf dieser Plattform nicht verfügbar.

journal.cli.offload.guidance.start = die Auslagerung wurde nicht gestartet.

journal.cli.offload.guidance.tool = verwend den installierten Befehl { $solstone } mit seinem mitgelieferten Sicherungswerkzeug.

journal.cli.offload.guidance.unfinished = die Auslagerung wurde nicht abgeschlossen. behalte ihre Wiederherstellungsdateien und sieh die aufgezeichneten Auswirkungen durch, bevor du es erneut versuchst.

journal.cli.offload.result.archive.failed = der Archivierungsschritt der Auslagerung wurde nicht abgeschlossen

journal.cli.offload.result.backup.failing = deine letzte Sicherung wurde nicht erfolgreich abgeschlossen

journal.cli.offload.result.backup.not.ready = deine Sicherung ist nicht eingerichtet oder hat noch keinen erfolgreichen Lauf abgeschlossen

journal.cli.offload.result.budget.already.satisfied = das Auslagerungsbudget war bereits erreicht, also musste nichts zur Freigabe markiert werden

journal.cli.offload.result.budget.satisfied = genug Medien wurden zur Freigabe markiert, um das Auslagerungsbudget zu erreichen

journal.cli.offload.result.confirm.failed = die Auslagerung konnte nicht bestätigen, dass die archivierten Dateien die Sicherung tatsächlich erreicht haben

journal.cli.offload.result.confirm.tool.failed = die Prüfung, die ein Auslagerungsarchiv bestätigt, konnte nicht abgeschlossen werden

journal.cli.offload.result.markable.media.exhausted = jede geeignete Datei wurde bereits geprüft; nichts Weiteres kann gerade zur Freigabe markiert werden

journal.cli.offload.result.other = ohne Grund gestoppt ({ $reason })

journal.cli.offload.result.unexpected.error = etwas Unerwartetes hat die Auslagerung unterbrochen

journal.cli.offload.result.verification.failed = die letzte Überprüfung der Sicherung hat ein Problem mit der Integrität der Sicherung gefunden

journal.cli.offload.result.verification.missing = noch keine Überprüfung der Sicherung wurde abgeschlossen

journal.cli.offload.result.verification.overdue = die Überprüfung der Sicherung ist nicht kürzlich genug gelungen

journal.cli.offload.stopped.archive.cancelled = die Auslagerung wurde gestoppt: der Archivvorgang wurde abgebrochen.

journal.cli.offload.stopped.archive.clock.changed = die Auslagerung wurde gestoppt: die Archivuhr ist rückwärts gegangen.

journal.cli.offload.stopped.archive.configuration.changed = die Auslagerung wurde gestoppt: die Archivkonfiguration hat sich geändert.

journal.cli.offload.stopped.archive.configuration.unavailable = die Auslagerung wurde gestoppt: die Archivkonfiguration konnte nicht gelesen werden.

journal.cli.offload.stopped.archive.credentials.expired = die Auslagerung wurde gestoppt: die Zugangsdaten des Archivs sind abgelaufen.

journal.cli.offload.stopped.archive.deadline.exceeded = die Auslagerung wurde gestoppt: die Frist des Archivvorgangs wurde erreicht.

journal.cli.offload.stopped.archive.invalid.credential.expiry = die Auslagerung wurde gestoppt: die Zugangsdaten des Archivs haben einen ungültigen Ablauf.

journal.cli.offload.stopped.archive.invalid.selection = die Auslagerung wurde gestoppt: Archivursprünge müssen eine nicht leere Auswahl absoluter Pfade sein.

journal.cli.offload.stopped.archive.journal.identity.changed = die Auslagerung wurde gestoppt: dein Journal konnte nicht bestätigt werden.

journal.cli.offload.stopped.archive.not.configured = die Auslagerung wurde gestoppt: die Archivsicherung ist nicht konfiguriert.

journal.cli.offload.stopped.archive.repository.identity.changed = die Auslagerung wurde gestoppt: die Sicherung stimmt nicht mehr überein.

journal.cli.offload.stopped.archive.repository.identity.invalid = die Auslagerung wurde gestoppt: die Sicherung konnte nicht identifiziert werden.

journal.cli.offload.stopped.archive.snapshot.identity.invalid = die Auslagerung wurde gestoppt: das Archiv hat keine vollständige Identität des Abbilds zurückgegeben.

journal.cli.offload.stopped.archive.source.unavailable = die Auslagerung wurde gestoppt: die vorbereiteten Archivdateien konnten nicht bestätigt werden.

journal.cli.offload.stopped.archive.tool.unavailable = die Auslagerung wurde gestoppt: ein Archivwerkzeug ist nicht verfügbar.

journal.cli.offload.stopped.creation = die Auslagerung wurde gestoppt: das Erstellen von Vorschlägen wurde nicht abgeschlossen.

journal.cli.offload.stopped.inputs.io = die Auslagerung wurde gestoppt: eine Datei in deinem Journal konnte nicht geöffnet oder gelesen werden.

journal.cli.offload.stopped.inputs.space = die Auslagerung wurde gestoppt: nicht genug freier Speicherplatz auf diesem Computer ({ $available } frei, { $required } benötigt).

journal.cli.offload.stopped.native.start = die Auslagerung wurde gestoppt: Archivstart abgelehnt (Status { $status }).

journal.cli.offload.stopped.plan.incomplete = die Auslagerung wurde gestoppt: das Vorbereiten der Vorschläge wurde nicht abgeschlossen.

journal.cli.offload.stopped.planning = die Auslagerung wurde gestoppt: die Auslagerung konnte ihre Vorschläge nicht vorbereiten.

journal.cli.offload.stopped.readback = die Auslagerung wurde gestoppt: das Zurücklesen der Kopie aus deiner Sicherung ist nicht gelungen.

journal.cli.offload.stopped.readback.credentials.unavailable = die Auslagerung wurde gestoppt: die Lesezugangsdaten des Archivs sind nicht verfügbar.

journal.cli.offload.stopped.readback.incomplete.discovery = die Auslagerung wurde gestoppt: der Archivdurchlauf hat nicht jeden Knoten erfasst.

journal.cli.offload.stopped.readback.invalid.selection = die Auslagerung wurde gestoppt: die ausgewählte Archivdatei ist ungültig.

journal.cli.offload.stopped.readback.native.failure = die Auslagerung wurde gestoppt: die archivierte Datei konnte nicht vorbereitet werden.

journal.cli.offload.stopped.readback.not.configured = die Auslagerung wurde gestoppt: das Lesen archivierter Dateien ist nicht konfiguriert.

journal.cli.offload.stopped.readback.snapshot.node.unavailable = die Auslagerung wurde gestoppt: die archivierte Datei konnte nicht bestätigt werden.

journal.cli.offload.stopped.readback.target.root.changed = die Auslagerung wurde gestoppt: die Datei konnte in deinem Journal nicht vorbereitet werden.

journal.cli.offload.stopped.readback.tool.changed = die Auslagerung wurde gestoppt: das installierte Archivwerkzeug hat sich geändert; versuch es noch einmal.

journal.cli.offload.stopped.readback.tool.failed = die Auslagerung wurde gestoppt: der Lesebefehl des Archivs ist nicht gelungen.

journal.cli.offload.stopped.readback.tool.unavailable = die Auslagerung wurde gestoppt: das installierte Archivwerkzeug ist nicht verfügbar.

journal.cli.offload.stopped.readback.unsupported = die Auslagerung wurde gestoppt: das Lesen archivierter Dateien ist auf dieser Plattform nicht verfügbar.

journal.cli.offload.stopped.readback.unsupported.backend = die Auslagerung wurde gestoppt: dieses Sicherungsziel unterstützt das Lesen archivierter Dateien nicht.

journal.cli.offload.stopped.readback.unsupported.tool.version = die Auslagerung wurde gestoppt: die Version des installierten Archivwerkzeugs wird nicht unterstützt.

journal.cli.offload.stopped.record = die Auslagerung wurde gestoppt: das Speichern des Datensatzes der Auslagerung ist nicht gelungen.

journal.cli.offload.stopped.selection = die Auslagerung wurde gestoppt: die ausgewählten Aufnahmen konnten nicht bestätigt werden.

journal.cli.offload.stopped.temporary.copy = die Auslagerung wurde gestoppt: das Erstellen einer temporären Kopie ist nicht gelungen.

journal.cli.offload.stopped.tool.reason = die Auslagerung wurde gestoppt: { $reason }.

journal.cli.offload.stopped.unfinished.work = die Auslagerung wurde gestoppt: dein Journal hat noch unabgeschlossene Arbeit.

journal.cli.reprocess.empty.range = keine Daten für die Tage { $day } bis { $through }

journal.cli.reprocess.facet.classification = { $path }: ungültige Facettenklassifikation

journal.cli.reprocess.facet.utf8 = nicht-{ $encoding }-Facette

journal.cli.reprocess.help = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $line_break }Einen vergangenen Journal-Tag zur Neuverarbeitung einreichen{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $day_column }Vergangener Tag im Format { $date_format }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_9 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $through_option }{ $gap_2 }Inklusives Bereichsende im Format { $date_format }{ $line_break }{ $syntax_continuation_13 }{ $line_break }{ $indent_2 }{ $unit_option }{ $gap_8 }Einen erneuten Versuch eines fehlgeschlagenen täglichen Verarbeitungsschritts anfordern{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_6 }Facette dieser Einheit; erforderlich, wenn die Einheit eine Facette hat{ $line_break }{ $indent_2 }{ $from_scratch_option }{ $gap_5 }Einen vollständigen täglichen Neulauf erzwingen und Markierungen behalten (ohne{ $line_break }                     den Tag als aktualisiert zu kennzeichnen){ $line_break }{ $indent_2 }{ $mark_updated_option }{ $gap_5 }Den Tag als mit neuen Rohdaten kennzeichnen, damit die tägliche Verarbeitung{ $line_break }                     ihn erneut in die Warteschlange stellt, und dann einen Drain anstoßen{ $line_break }{ $indent_2 }{ $owed_option }{ $gap_13 }Die täglichen Ausgaben auflisten, die am Tag oder im Bereich ausstehen, und warum,{ $line_break }                     ohne etwas einzureichen{ $line_break }{ $indent_2 }{ $json_option }{ $gap_13 }Mit { $owed_flag }, jeden Tag als Zustände und Anzahlen melden: seine{ $line_break }                     Markierungen, seine Abdeckung, wie viele tägliche Ausgaben er hat und{ $line_break }                     wie viele ausstehen, nach Ursache{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_6 }ausführliche Ausgabe einschalten{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_8 }Debug-Protokollierung einschalten

journal.cli.reprocess.input.day = ungültiger Tag: { $day }

journal.cli.reprocess.intent.busy = der angeforderte Tag gehört einem anderen Executor

journal.cli.reprocess.intent.invalid.day = ungültiger Arbeitstag

journal.cli.reprocess.intent.uncertain = die Veröffentlichung der Arbeitsabsicht ist ungewiss: { $outcome }

journal.cli.reprocess.intent.unrecognized = unbekannte Arbeitsabsicht

journal.cli.reprocess.native.arguments = nativ { $binary } { $verb } ist nicht gelungen: Argumente sind nicht gültig als { $encoding }

journal.cli.reprocess.no.data = keine Daten für den Tag { $day }

journal.cli.reprocess.no.engine = noch ist kein Modell gewählt. wähle eins unter Denken und versuch es erneut

journal.cli.reprocess.owed.internal = { $flag } meldet nur; verwend { $function }

journal.cli.reprocess.owed.only = { $owed_flag } meldet nur; nichts wurde eingereicht

journal.cli.reprocess.past.only = die Neuverarbeitung gilt nur für die Vergangenheit (heute oder ein zukünftiger Tag kann nicht neu verarbeitet werden)

journal.cli.reprocess.path.utf8 = nicht-{ $encoding }-Ursprungspfad

journal.cli.reprocess.range.plan = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] Plan der Neuverarbeitung von Grund auf:{ $break }{ $days } Tag mit Daten ({ $segments } Segment) wird in die Warteschlange gestellt. Der Fortschritt ist in { $top } oder { $health } sichtbar. Anfragen überstehen einen Neustart des Supervisors; ein Tag, der nicht von Grund auf fertig werden kann, endet als gewöhnliche Neuverarbeitung.{ $break }Dieser Tag kann Stunden dauern; die heutige Arbeit kommt zwischen den Aufgaben zuerst.{ $break }erneut mit { $yes } ausführen, um fortzufahren
       *[other] Plan der Neuverarbeitung von Grund auf:{ $break }{ $days } Tag mit Daten ({ $segments } Segmente) wird in die Warteschlange gestellt. Der Fortschritt ist in { $top } oder { $health } sichtbar. Anfragen überstehen einen Neustart des Supervisors; ein Tag, der nicht von Grund auf fertig werden kann, endet als gewöhnliche Neuverarbeitung.{ $break }Dieser Tag kann Stunden dauern; die heutige Arbeit kommt zwischen den Aufgaben zuerst.{ $break }erneut mit { $yes } ausführen, um fortzufahren
    }
   *[other] { $segments_quantity ->
        [one] Plan der Neuverarbeitung von Grund auf:{ $break }{ $days } Tage mit Daten ({ $segments } Segment) werden in die Warteschlange gestellt. Der Fortschritt ist in { $top } oder { $health } sichtbar. Anfragen überstehen einen Neustart des Supervisors; ein Tag, der nicht von Grund auf fertig werden kann, endet als gewöhnliche Neuverarbeitung.{ $break }Diese Tage können Stunden dauern; die heutige Arbeit kommt zwischen den Aufgaben zuerst.{ $break }erneut mit { $yes } ausführen, um fortzufahren
       *[other] Plan der Neuverarbeitung von Grund auf:{ $break }{ $days } Tage mit Daten ({ $segments } Segmente) werden in die Warteschlange gestellt. Der Fortschritt ist in { $top } oder { $health } sichtbar. Anfragen überstehen einen Neustart des Supervisors; ein Tag, der nicht von Grund auf fertig werden kann, endet als gewöhnliche Neuverarbeitung.{ $break }Diese Tage können Stunden dauern; die heutige Arbeit kommt zwischen den Aufgaben zuerst.{ $break }erneut mit { $yes } ausführen, um fortzufahren
    }
}

journal.cli.reprocess.range.saved = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] gespeicherte Anfragen zur Neuverarbeitung von Grund auf für { $days } Tag ({ $segments } Segment){ $break }der Fortschritt ist im Zustand sichtbar; die heutige Arbeit kommt zuerst
       *[other] gespeicherte Anfragen zur Neuverarbeitung von Grund auf für { $days } Tag ({ $segments } Segmente){ $break }der Fortschritt ist im Zustand sichtbar; die heutige Arbeit kommt zuerst
    }
   *[other] { $segments_quantity ->
        [one] gespeicherte Anfragen zur Neuverarbeitung von Grund auf für { $days } Tage ({ $segments } Segment){ $break }der Fortschritt ist im Zustand sichtbar; die heutige Arbeit kommt zuerst
       *[other] gespeicherte Anfragen zur Neuverarbeitung von Grund auf für { $days } Tage ({ $segments } Segmente){ $break }der Fortschritt ist im Zustand sichtbar; die heutige Arbeit kommt zuerst
    }
}

journal.cli.reprocess.record.invalid = Fehlerhafter Datensatz der täglichen Einheit: ungültige Identität der täglichen Einheit

journal.cli.reprocess.record.io = E/A-Fehler: { $cause }

journal.cli.reprocess.record.mismatch = Fehlerhafter Datensatz der täglichen Einheit: Identitätsabweichung im Datensatz unter { $path }: erwartet { $expected }, gefunden { $found }

journal.cli.reprocess.retry.accepted = diese tägliche Ausgabe ist bereits angenommen

journal.cli.reprocess.retry.disabled = das tägliche Talent ist nicht mehr aktiviert

journal.cli.reprocess.retry.invalid.day = ungültiger Tag für den erneuten Versuch

journal.cli.reprocess.retry.invalid.unit = ungültige Einheit für den erneuten Versuch

journal.cli.reprocess.retry.missing = die tägliche Einheit fehlt

journal.cli.reprocess.retry.missing.evidence = zurückgehaltener Nachweis fehlt

journal.cli.reprocess.retry.no.longer.held = die Einheit ist nicht mehr zurückgehalten

journal.cli.reprocess.retry.not.daily = nicht tägliche Arbeit

journal.cli.reprocess.retry.state = diese tägliche Einheit kann nicht erneut versucht werden, ohne ihren aktuellen Zustand zu klären

journal.cli.reprocess.schedule.lookback = Rückblick des Tageszeitplans außerhalb von { $range }

journal.cli.reprocess.schema.missing = der Pfad des Schema-Prompts fehlt

journal.cli.reprocess.schema.name = das Schema muss ein Dateiname sein

journal.cli.reprocess.shape.invalid = ungültiges Shape für { $path }

journal.cli.reprocess.shape.object = das Shape-Sidecar ist kein Objekt

journal.cli.reprocess.shape.written = ungültiges geschriebenes Ursprungs-Shape: { $path }

journal.cli.reprocess.source.json.object = { $path } ist kein { $encoding }-Objekt

journal.cli.reprocess.source.line.error = { $path } Zeile { $line }: { $error }

journal.cli.reprocess.source.line.object = { $path } Zeile { $line } ist kein Objekt

journal.cli.reprocess.source.symlink = Tagesquelle ist ein symbolischer Link: { $path }

journal.cli.reprocess.source.utf8 = nicht-{ $encoding }-Pfad der Tagesquelle

journal.cli.reprocess.talent.overrides = Talent-Überschreibungen aus { $path } konnten nicht gelesen werden

journal.cli.reprocess.template.utf8 = nicht-{ $encoding }-Vorlagenname

journal.cli.reprocess.through.order = { $through_flag } muss am Starttag oder danach liegen

journal.cli.reprocess.through.required = { $through_flag } erfordert { $from_scratch_flag }

journal.cli.reprocess.unit.failed = Neuverarbeitungseinheit ist fehlgeschlagen: { $error }

journal.cli.reprocess.unit.saved = Wiederholungsanfrage für { $unit } am { $day } gespeichert

journal.cli.reprocess.unreadable = { $day }  (nicht lesbar: { $error })

journal.cli.reprocess.usage.conflict = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: Fehler: Argument { $option }: nicht zulässig mit Argument { $previous }

journal.cli.reprocess.usage.missing.day = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: Fehler: folgende Argumente sind erforderlich: { $positional }

journal.cli.reprocess.usage.missing.value = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: Fehler: Argument { $option }: genau ein Argument erwartet

journal.cli.reprocess.usage.requires = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: Fehler: Argument { $option }: erfordert { $required }

journal.cli.reprocess.usage.unknown = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.reprocess.waiting = Anfrage gespeichert; starte den Hintergrunddienst mit { $start_command }, um sie zu verarbeiten

journal.cli.retention.proposal.eligible = Richtlinien-Eignung: berechtigt { $open_brace } Anker: { $anchor }, age_days: { $age_days }, Zeitraum: Tage({ $period_days }) { $close_brace }

journal.cli.retention.proposal.forever = Richtlinien-Eignung: für immer behalten

journal.cli.retention.proposal.no.anchor = Richtlinien-Eignung: Anker fehlt { $open_brace } Anker: { $anchor } { $close_brace }

journal.cli.retention.proposal.unrecorded = das Entfernen ist fehlgeschlagen, bevor ein Vorschlag aufgezeichnet wurde

journal.cli.retention.proposal.young = Richtlinien-Eignung: zu jung { $open_brace } Anker: { $anchor }, age_days: { $age_days }, Zeitraum: Tage({ $period_days }) { $close_brace }

journal.cli.retention.recovery.activity.binding = dein Journal konnte nicht geprüft werden, daher wurde dieses Löschen angehalten

journal.cli.retention.recovery.activity.busy = dein Journal ist mit einem anderen Auftrag beschäftigt

journal.cli.retention.recovery.activity.hold = ein anderer Journal-Vorgang muss beachtet werden, bevor dieses Entfernen weitergehen kann

journal.cli.retention.recovery.activity.reserved = dieser Ort ist für einen Journal-Vorgang reserviert und kann nicht entfernt werden

journal.cli.retention.recovery.path.directory = dort ist ein Ordner, wo eine Aufnahme erwartet wurde

journal.cli.retention.recovery.path.invalid = der Ort benennt keine Datei in deinem Journal

journal.cli.retention.recovery.path.outside = das liegt nicht in deinem Journal, daher wurde es in Ruhe gelassen

journal.cli.retention.recovery.path.permission = die Datei konnte nicht gelesen oder entfernt werden, was gewöhnlich ein Berechtigungsproblem bedeutet

journal.cli.retention.recovery.path.unavailable = die Datei konnte nicht entfernt werden; das Journal blieb unverändert

journal.cli.retention.recovery.record.prepare = der Datensatz dieses Entfernens konnte nicht vorbereitet werden

journal.cli.retention.recovery.record.write = der Datensatz dieses Entfernens konnte nicht geschrieben werden

journal.cli.retention.recovery.segment.busy = ein anderer Prozess arbeitet an diesem Segment

journal.cli.retention.recovery.segment.gone = dieses Segment wurde bereits entfernt

journal.cli.retention.recovery.segment.live.occupied = etwas Neues wurde dorthin geschrieben, wo dieses Segment war, daher wurde es beiseitegelegt, statt überschrieben zu werden

journal.cli.retention.recovery.segment.missing = so ein Segment gibt es nicht in deinem Journal

journal.cli.retention.recovery.segment.staged = dieses Segment ist mitten im Entfernen und beiseitegelegt; es wird nicht mit deinen anderen Segmenten aufgeführt, bis es fertig ist

journal.cli.retention.recovery.segment.unconfirmed = es konnte nicht bestätigt werden, dass es weg ist, daher wird es nicht als entfernt gemeldet

journal.cli.retention.recovery.segment.unfinished = ein früheres Entfernen dieses Segments wurde nicht abgeschlossen; es muss angeschaut werden, bevor dies erneut versucht werden kann

journal.cli.retention.removal.uncertain = das Löschen konnte nicht abgeschlossen oder bestätigt werden. Wiederherstellung ist nötig.

journal.cli.schedule.cell.disabled = deaktiviert

journal.cli.schedule.cell.invalid = ungültig

journal.cli.schedule.cell.midnight = Mitternacht

journal.cli.schedule.cell.never = nie

journal.cli.schedule.cell.now = jetzt

journal.cli.schedule.config = Konfiguration: { $path }

journal.cli.schedule.empty = Keine Zeitpläne konfiguriert.{ $line_break }{ $line_break }Füge Zeitpläne hinzu in: { $path }

journal.cli.schedule.error.cmd.invalid = Zeitplan '{ $name }' hat einen ungültigen Befehl

journal.cli.schedule.error.config.malformed = Zeitplankonfiguration unter { $path } ist fehlerhaft

journal.cli.schedule.error.config.not.object = Zeitplankonfiguration unter { $path } muss ein JSON-Objekt sein

journal.cli.schedule.error.config.unreadable = Zeitplankonfiguration unter { $path } ist nicht lesbar

journal.cli.schedule.error.entry.invalid = Zeitplan '{ $name }' ist kein gültiger Eintrag

journal.cli.schedule.error.state.malformed = Zeitplaner-Zustand unter { $path } ist fehlerhaft

journal.cli.schedule.error.state.not.object = Zeitplaner-Zustand unter { $path } muss ein JSON-Objekt sein

journal.cli.schedule.error.state.unreadable = Zeitplaner-Zustand unter { $path } ist nicht lesbar

journal.cli.schedule.header.cmd = BEFEHL

journal.cli.schedule.header.every = ALLE

journal.cli.schedule.header.last.run = LETZTER LAUF

journal.cli.schedule.header.name = NAME

journal.cli.schedule.header.next.due = NÄCHSTE FÄLLIGKEIT

journal.cli.schedule.tag.disabled = [deaktiviert]

journal.cli.schedule.weekly.friday = Freitag { $time }

journal.cli.schedule.weekly.monday = Montag { $time }

journal.cli.schedule.weekly.saturday = Samstag { $time }

journal.cli.schedule.weekly.sunday = Sonntag { $time }

journal.cli.schedule.weekly.thursday = Donnerstag { $time }

journal.cli.schedule.weekly.tuesday = Dienstag { $time }

journal.cli.schedule.weekly.wednesday = Mittwoch { $time }

journal.cli.segment.argument.missing = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: folgende Argumente sind erforderlich: { $input }

journal.cli.segment.argument.unknown = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.segment.check.backward = Rückwärtskette

journal.cli.segment.check.chain.no.day = { $prev_segment_key } gesetzt ohne { $prev_day_key }

journal.cli.segment.check.chain.no.successor = nächstes Segment nicht gefunden, kein Datenstromende

journal.cli.segment.check.chain.previous = vorheriges Segment gefunden

journal.cli.segment.check.chain.start = kein vorheriges Segment

journal.cli.segment.check.chain.tail = Datenstromende

journal.cli.segment.check.content.empty = kein { $audio_file }, { $screen_file } oder { $browser_files }

journal.cli.segment.check.content.present = Inhaltsdateien vorhanden

journal.cli.segment.check.directory.absent = Segmentverzeichnis fehlt

journal.cli.segment.check.directory.exists = Verzeichnis existiert

journal.cli.segment.check.directory.missing = Verzeichnis fehlt

journal.cli.segment.check.forward = Vorwärtskette

journal.cli.segment.check.index = Indexvorhandensein

journal.cli.segment.check.index.absent = Journalindex nicht verfügbar

journal.cli.segment.check.index.error = Journalindex-Fehler: { $error } (ausführen: { $index_command })

journal.cli.segment.check.index.missing = Segment nicht indiziert

journal.cli.segment.check.index.present = Segment indiziert

journal.cli.segment.check.marker.exists = { $stream_file } existiert

journal.cli.segment.check.marker.invalid = { $stream_file } ungültiges { $json_format }

journal.cli.segment.check.marker.missing = { $stream_file } fehlt

journal.cli.segment.check.marker.no.stream = { $stream_file } ohne Datenstromfeld

journal.cli.segment.check.marker.valid = { $stream_file } gültig

journal.cli.segment.check.next = nächstes Segment { $path }

journal.cli.segment.check.previous.missing = fehlendes vorheriges Segment { $path }

journal.cli.segment.day.empty = Keine Segmente für { $day } gefunden

journal.cli.segment.guard.activity = ein anderer Journal-Vorgang muss beachtet werden, bevor diese Änderung weitergehen kann

journal.cli.segment.guard.binding = das Journal konnte nicht geprüft werden, daher ist diese Änderung zurückgehalten

journal.cli.segment.guard.reserved = dieser Ort ist für einen Journal-Vorgang reserviert und kann nicht geändert werden

journal.cli.segment.help.inspect = Verwendung: { $syntax }{ $break }{ $break }Positionsargumente:{ $break }{ $option_3 }Segmentpfad: { $example_path } (z. B.{ $break }{ $syntax_line_4 }{ $break }{ $break }Optionen:{ $break }{ $option_7 }diese Hilfe anzeigen und beenden{ $break }{ $option_8 }Als { $json_format } ausgeben

journal.cli.segment.help.list = Verwendung: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $break }Positionsargumente:{ $break }{ $option_5 }Tag im Format { $day_format }{ $break }{ $break }Optionen:{ $break }{ $option_8 }diese Hilfe anzeigen und beenden{ $break }{ $option_9 }Auf einen bestimmten Datenstrom filtern{ $break }{ $option_10 }Als { $json_format } ausgeben

journal.cli.segment.help.move = Verwendung: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $syntax_line_3 }{ $break }{ $break }Positionsargumente:{ $break }{ $option_6 }Segmentpfad: { $example_path } (z. B.{ $break }{ $syntax_line_7 }{ $break }{ $break }Optionen:{ $break }{ $option_10 }diese Hilfe anzeigen und beenden{ $break }{ $option_11 }Zieltag ({ $day_format }){ $break }{ $option_12 }Neue Uhrzeit ({ $time_format }), ursprüngliche Dauer beibehalten{ $break }{ $option_13 }Plan anzeigen, ohne Änderungen vorzunehmen{ $break }{ $option_14 }Ausführlichen Fortschritt anzeigen

journal.cli.segment.help.top = Verwendung: { $syntax }{ $break }{ $break }Journal-Segmente ansehen und verwalten{ $break }{ $break }Positionsargumente:{ $break }{ $syntax_line_5 }{ $break }{ $option_6 }Segmente eines Tags auflisten{ $break }{ $option_7 }Segment-Metadaten anzeigen{ $break }{ $option_8 }Segmentintegrität überprüfen{ $break }{ $option_9 }Segment auf einen anderen Tag oder eine andere Uhrzeit verschieben{ $break }{ $break }Optionen:{ $break }{ $option_12 }diese Hilfe anzeigen und beenden{ $break }{ $option_13 }ausführliche Ausgabe einschalten{ $break }{ $option_14 }Debug-Protokollierung einschalten

journal.cli.segment.help.verify = Verwendung: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $break }Positionsargumente:{ $break }{ $option_4 }Segmentpfad: { $example_path }{ $break }{ $break }Optionen:{ $break }{ $option_7 }diese Hilfe anzeigen und beenden{ $break }{ $option_8 }Alle Segmente eines Tags überprüfen{ $break }{ $option_9 }Als { $json_format } ausgeben

journal.cli.segment.identity.default = Segmentliste abgelehnt: benanntes Datenstromverzeichnis "{ $default_stream }" lässt sich nicht als Datensatzidentität schreiben: { $path }

journal.cli.segment.identity.duplicate = Segmentliste abgelehnt: mehrere Segmente teilen sich Datenstrom { $stream } und Schlüssel { $key }

journal.cli.segment.identity.encoding = Segmentliste abgelehnt: Segmentpfad ist nicht in { $encoding } darstellbar: { $path }

journal.cli.segment.identity.refused = Segmentliste abgelehnt: { $detail }

journal.cli.segment.inspect.chain = Kette:

journal.cli.segment.inspect.events = { $quantity ->
    [one] Ereignisse: { $entries } Eintrag
   *[other] Ereignisse: { $entries } Einträge
}

journal.cli.segment.inspect.events.tracts = { $quantity ->
    [one] Ereignisse: { $entries } Eintrag ({ $tracts })
   *[other] Ereignisse: { $entries } Einträge ({ $tracts })
}

journal.cli.segment.inspect.files = Dateien ({ $count }):

journal.cli.segment.inspect.index.error = Index: Fehler ({ $error }) - ausführen: { $index_command }

journal.cli.segment.inspect.index.missing = Index: nicht indiziert

journal.cli.segment.inspect.index.unavailable = Index: nicht verfügbar

journal.cli.segment.inspect.indexed = { $quantity ->
    [one] Index: indiziert ({ $chunks } Block)
   *[other] Index: indiziert ({ $chunks } Blöcke)
}

journal.cli.segment.inspect.next = nächstes: { $next }

journal.cli.segment.inspect.next.none = nächstes: (keins)

journal.cli.segment.inspect.next.tail = nächstes: (Datenstromende)

journal.cli.segment.inspect.path = Segment: { $path }

journal.cli.segment.inspect.prev = vorheriges: { $previous }

journal.cli.segment.inspect.prev.missing = vorheriges: { $path } [FEHLT]

journal.cli.segment.inspect.prev.none = vorheriges: (keins)

journal.cli.segment.inspect.size = Größe: { $size }

journal.cli.segment.inspect.stream = Datenstrom:  { $stream }

journal.cli.segment.inspect.stream.seq = Datenstrom:  { $stream } (Folge { $sequence })

journal.cli.segment.inspect.talents = Talente ({ $count }):

journal.cli.segment.inspect.time = Zeit:    { $range } ({ $seconds }s)

journal.cli.segment.list.heading.dur = DAUER

journal.cli.segment.list.heading.files = DATEIEN

journal.cli.segment.list.heading.segment = SEGMENT

journal.cli.segment.list.heading.size = GRÖSSE

journal.cli.segment.list.heading.stream = DATENSTROM

journal.cli.segment.list.heading.talents = TALENTE

journal.cli.segment.list.heading.time = ZEIT

journal.cli.segment.missing = Segment nicht gefunden: { $path }

journal.cli.segment.move.day.format = Ungültiges { $to_day_flag }-Format: { $to_day } (erwartet { $date_format })

journal.cli.segment.move.directory.created = Verzeichnis erstellt: { $path }

journal.cli.segment.move.directory.exists = Ziel { $destination } existiert bereits; keine Änderungen vorgenommen

journal.cli.segment.move.directory.failed = Schritt 1 Verschieben des Verzeichnisses ist fehlgeschlagen: { $error }; Ursprung und Index bleiben maßgeblich

journal.cli.segment.move.directory.moved = Verzeichnis verschoben: { $source } -> { $destination }

journal.cli.segment.move.dry = [Probelauf] Keine Änderungen vorgenommen

journal.cli.segment.move.events.none = kein { $event_file } zum Umschreiben

journal.cli.segment.move.events.rewritten = { $quantity ->
    [one] { $count } { $event_file }-Zeile umgeschrieben (Tag: { $source_day }->{ $destination_day }, Segment: { $source }->{ $destination })
   *[other] { $count } { $event_file }-Zeilen umgeschrieben (Tag: { $source_day }->{ $destination_day }, Segment: { $source }->{ $destination })
}

journal.cli.segment.move.exists = Segment { $segment } existiert bereits auf { $day }/{ $stream }. Verwende { $to_time_flag }, um eine andere Uhrzeit anzugeben.

journal.cli.segment.move.fail.ambiguous = { $quantity ->
    [one] Schritt 3 Nachfolger-Anpassung nach dem Verschieben ist inkonsistent: mehrdeutige Kette: { $count } Nachfolger blieb unangepasst
   *[other] Schritt 3 Nachfolger-Anpassung nach dem Verschieben ist inkonsistent: mehrdeutige Kette: { $count } Nachfolger blieben unangepasst
}

journal.cli.segment.move.fail.events = Schritt 2 Umschreiben der Ereignisse nach dem Verschieben ist inkonsistent: { $detail }

journal.cli.segment.move.fail.health.marker = Schritt 6 Berühren der Zustandsmarkierung nach dem Verschieben ist inkonsistent: { $detail }

journal.cli.segment.move.fail.search = Schritt 5 Suchaktualisierung nach dem Verschieben ist inkonsistent: die Suchaktualisierung konnte nicht in die Warteschlange gestellt werden: { $error }

journal.cli.segment.move.fail.successor = Schritt 3 Nachfolger-Anpassung nach dem Verschieben ist inkonsistent: { $detail }

journal.cli.segment.move.fail.tail.locked = Schritt 4 Reparatur des Datenstromendes nach dem Verschieben ist inkonsistent: Datenstrom-Datensatz konnte nicht gesperrt werden

journal.cli.segment.move.fail.tail.malformed = Schritt 4 Reparatur des Datenstromendes nach dem Verschieben ist inkonsistent: fehlerhafter Datenstrom-Datensatz

journal.cli.segment.move.fail.tail.no.record = Schritt 4 Reparatur des Datenstromendes nach dem Verschieben ist inkonsistent: kein Datenstrom-Datensatz; Identität wurde nicht neu verfasst

journal.cli.segment.move.fail.tail.write = Schritt 4 Reparatur des Datenstromendes nach dem Verschieben ist inkonsistent: atomare Veröffentlichung ist fehlgeschlagen

journal.cli.segment.move.markers.touched = Zustandsmarkierungen berührt: { $days }

journal.cli.segment.move.no.duration = Schlüssel des Ursprungssegments hat keine Dauer

journal.cli.segment.move.no.marker = Kein { $stream_file } im Ursprungssegment

journal.cli.segment.move.no.slot = Kein verfügbarer Segmentplatz nahe { $destination_segment } am { $to_day }

journal.cli.segment.move.plan = Verschieben: { $source } -> { $destination }

journal.cli.segment.move.plan.ambiguous = { $quantity ->
    [one] Nachfolger zum Anpassen: (mehrdeutig: { $count } Nachfolger)
   *[other] Nachfolger zum Anpassen: (mehrdeutig: { $count } Nachfolger)
}

journal.cli.segment.move.plan.events = { $event_file }-Zeilen: { $count }

journal.cli.segment.move.plan.index = Indexblöcke: { $chunks }

journal.cli.segment.move.plan.index.error = Fehler beim Lesen des Index: { $error } (Löschen+Neuindizieren wird versucht; ausführen: { $index_command })

journal.cli.segment.move.plan.markers = Zustandsmarkierungen: { $source_day }, { $destination_day }

journal.cli.segment.move.plan.successor = Nachfolger zum Anpassen: { $path }

journal.cli.segment.move.plan.tail = Nachfolger zum Anpassen: (keiner - Datenstromende)

journal.cli.segment.move.running = Verschieben wird ausgeführt...

journal.cli.segment.move.same = Ursprung und Ziel sind gleich

journal.cli.segment.move.search.queued = Suchaktualisierungen in der Warteschlange

journal.cli.segment.move.stream.mismatch = Datenstrom stimmt nicht überein: Pfad sagt '{ $path_stream }', aber { $stream_file } sagt '{ $marker_stream }'

journal.cli.segment.move.stream.rebuilt = Datenstromzustand neu aufgebaut: { $stream }

journal.cli.segment.move.successor.none = kein Nachfolger zum Anpassen (Datenstromende)

journal.cli.segment.move.successor.patched = angepasster Nachfolger { $path }

journal.cli.segment.move.talents = { $command } wird tägliche Talente an den berührten Tagen erneut ausführen

journal.cli.segment.move.time.format = Ungültiges { $to_time_flag }-Format: { $time } (erwartet { $time_format })

journal.cli.segment.path.syntax = Segmentpfad muss { $path_form } sein (z. B. { $example })

journal.cli.segment.relocation.deleted = Segment wurde von der besitzenden Person gelöscht

journal.cli.segment.relocation.location = der Ort des Segments stimmt nicht mit dem geplanten Verschieben überein

journal.cli.segment.relocation.marker = Markierung konnte nicht gelesen werden

journal.cli.segment.status.pass = BESTANDEN

journal.cli.segment.verify.day.result = { $quantity ->
    [one] Zusammenfassung: { $passed }/{ $total } Prüfung bestanden
   *[other] Zusammenfassung: { $passed }/{ $total } Prüfungen bestanden
}

journal.cli.segment.verify.input = { $verb } erfordert einen Segmentpfad oder { $day_flag }

journal.cli.segment.verify.result = { $quantity ->
    [one] { $passed }/{ $total } Prüfung bestanden
   *[other] { $passed }/{ $total } Prüfungen bestanden
}

journal.cli.share.activities = { $facet }-Aktivitäten für { $day } konnten nicht gelesen werden: { $detail }

journal.cli.share.activity.missing = keine Aktivität '{ $activity }' in { $facet } am { $day }

journal.cli.share.activity.unmatched = Aktivität '{ $activity }' stimmt nicht mit { $person } überein

journal.cli.share.facets = Facetten konnten nicht aufgelistet werden: { $detail }

journal.cli.share.failure = { $command }: { $detail }

journal.cli.share.help.match = Verwendung: { $syntax }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }Entitäts-ID oder ein eindeutiger Name oder Alias einer Person{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }der zu lesende Tag{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }nur diese Facette (Standard: jede Facette)

journal.cli.share.help.package = Verwendung: { $syntax }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }Entitäts-ID oder ein eindeutiger Name oder Alias einer Person{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }der Tag der Aktivität{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }die Facette der Aktivität{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_5 }die Aktivitäts-ID

journal.cli.share.help.root = Verwendung: { $syntax }{ $line_break }{ $line_break }Finde Aktivitäten, die mit einer verknüpften Person geteilt werden, und zeige eine Vorschau, was geteilt würde.{ $line_break }Beide Unterbefehle lesen nur das Journal.{ $line_break }{ $line_break }Befehle:{ $line_break }{ $match_command }Aktivitäten des Tages auflisten, die mit einer Person übereinstimmen, je eine { $json }-Zeile{ $line_break }{ $package_command }eingegrenztes Zusammenfassungspaket für eine übereinstimmende Aktivität ausgeben

journal.cli.share.parse.argument = { $command }: Fehler: unbekanntes Argument '{ $argument }'

journal.cli.share.parse.duplicate = { $command }: Fehler: { $flag } mehr als einmal angegeben

journal.cli.share.parse.encoding = { $command }: Fehler: Argumente müssen gültiges { $encoding } sein

journal.cli.share.parse.missing = { $command }: Fehler: ein Unterbefehl ist erforderlich

journal.cli.share.parse.required = { $command }: Fehler: { $option } ist erforderlich

journal.cli.share.parse.unknown = { $command }: Fehler: unbekannter Unterbefehl '{ $argument }'

journal.cli.share.parse.value = { $command }: Fehler: { $flag } braucht einen Wert

journal.cli.share.provider.activity.unavailable = Aktivitätszusammenfassung nicht verfügbar: { $detail }

journal.cli.share.provider.child.cleanup = { $primary } (Aufräumen: { $cleanup })

journal.cli.share.provider.child.exit = Austritt { $code }

journal.cli.share.provider.child.exit.signal = Austritt { $code } Signal { $signal }

journal.cli.share.provider.child.invalid = ungültige Kindprozess-Antwort ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process = Prozess-E/A fehlgeschlagen: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process.cleanup = Prozess-E/A fehlgeschlagen: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }; { $cleanup_field }{ $cleanup }

journal.cli.share.provider.child.protocol = Protokoll { $reason } ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.signal = Signal { $signal }

journal.cli.share.provider.child.status.unknown = Status unbekannt

journal.cli.share.provider.child.truncated = { $bytes } [abgeschnitten]

journal.cli.share.provider.child.unavailable = nicht verfügbar

journal.cli.share.provider.child.unexpected = unerwarteter Kindprozess ({ $status }); { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.entities.read = Entitäten konnten nicht gelesen werden: { $detail }

journal.cli.share.provider.entity.invalid = Journal-Entität { $entity } konnte nicht geladen werden: { $detail }

journal.cli.share.provider.matching = Abgleich { $facet }/{ $day }/{ $activity } für { $entity }

journal.cli.share.provider.model.unconfigured = Es ist noch kein Modell ausgewählt. Wähle eines in Denken.

journal.cli.share.provider.person.absent = keine Person in diesem Journal heißt '{ $query }'

journal.cli.share.provider.person.ambiguous = '{ $query }' benennt mehr als eine Person ({ $people }); verwende eine Entitäts-ID

journal.cli.share.provider.presence.unavailable = Anwesenheitsprüfung nicht verfügbar: { $detail }

journal.cli.share.provider.segment.unavailable = Segmentzusammenfassung nicht verfügbar: { $detail }

journal.cli.share.provider.talent.invalid = Talent '{ $talent }' gab ungültiges { $json } zurück: { $error }

journal.cli.share.provider.talent.prepare = Talent '{ $talent }': { $detail }

journal.cli.share.provider.talent.timeout.numeric = Konfiguration '{ $talent }' deklariert { $timeout_s_field } nichtnumerisch

journal.cli.share.provider.talent.timeout.range = Konfiguration '{ $talent }' deklariert { $timeout_s_field } außerhalb des unterstützten Bereichs

journal.cli.share.provider.talent.tokens = Talent '{ $talent }' deklariert { $max_output_tokens_field } nicht

journal.cli.share.provider.talent.type = nicht unterstützter Talent-Typ: { $kind }

journal.cli.share.provider.talent.unfinished = Talent '{ $talent }' wurde nicht fertig: { $outcome }

journal.cli.share.startup.failed = { $command } ist fehlgeschlagen: { $detail }

journal.cli.share.startup.runtime.current = Worker-Programmdatei konnte nicht aufgelöst werden: { $error }

journal.cli.share.startup.runtime.parent = Worker-Programmdatei hat kein übergeordnetes Verzeichnis: { $path }

journal.cli.share.startup.runtime.root = Installationswurzel konnte nicht aus dem Verzeichnis der Programmdatei aufgelöst werden: { $path }

journal.cli.share.startup.sibling.missing = { $command } ist fehlgeschlagen: gleichgeordnete Programmdatei { $path } fehlt

journal.cli.share.startup.sibling.parent = { $command } ist fehlgeschlagen: aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.share.transport.child.inspect = Kindprozess vor dem Beenden untersuchen: { $detail }

journal.cli.share.transport.child.reap = Kindprozess einsammeln: { $detail }

journal.cli.share.transport.child.retry = Kindprozess erneut einsammeln: { $detail }

journal.cli.share.transport.child.terminate = Kindprozess beenden: { $detail }

journal.cli.share.transport.collector = { $pipe }-Sammler: { $detail }

journal.cli.share.transport.collector.panic = { $pipe }-Sammler: Thread-Panic

journal.cli.share.transport.field.array = { $field } muss ein Array oder null sein

journal.cli.share.transport.field.boolean = { $field } muss ein Wahrheitswert sein

journal.cli.share.transport.field.optional = { $field } muss eine Zeichenkette oder null sein

journal.cli.share.transport.field.string = { $field } muss eine Zeichenkette sein

journal.cli.share.transport.field.strings = { $field } darf nur Zeichenketten enthalten

journal.cli.share.transport.object = Datensatz muss ein { $json }-Objekt sein

journal.cli.share.transport.outcome = Ergebnis der Antwort ist nicht unterstützt

journal.cli.share.transport.pipe.missing = Leitung { $pipe } ist nicht verfügbar

journal.cli.share.transport.request.empty = { $contents } darf nicht leer sein

journal.cli.share.transport.request.overflow = { $overflow } erfordert, dass der erste Inhaltsteil Text ist

journal.cli.share.transport.schema = Schema des Datensatzes ist nicht unterstützt

journal.cli.share.transport.stdout.limit = { $stdout } abgeschnitten bei { $limit } Bytes

journal.cli.share.transport.usage = { $usage } ist erforderlich

journal.cli.share.transport.worker.channel = Abschlusskanal des Pipe-Workers: { $detail }

journal.cli.share.transport.worker.join = Join des { $pipe }-Workers: Thread-Panic

journal.cli.share.transport.worker.start = der Start des { $pipe }-Workers: { $detail }

journal.cli.share.transport.writer = { $stdin }-Schreiber: { $detail }

journal.cli.share.transport.writer.panic = { $stdin }-Schreiber: Thread-Panic

journal.cli.share.usage.match = Verwendung: { $syntax }

journal.cli.share.usage.package = Verwendung: { $syntax }

journal.cli.share.usage.root = Verwendung: { $syntax }

journal.cli.social.failed = der Abschluss ist nicht gelungen: { $detail }

journal.cli.social.follow.done = du folgst dem Space, den { $sharer } mit dir geteilt hat.

journal.cli.social.receive.added = { $activity_quantity ->
    [one] { $sharer } hat { $activities } neue Aktivität mit dir geteilt.
   *[other] { $sharer } hat { $activities } neue Aktivitäten mit dir geteilt.
}

journal.cli.social.receive.failed = der Space, den { $sharer } mit dir geteilt hat, konnte nicht gelesen werden: { $detail }

journal.cli.social.receive.none = nichts Neues von { $sharer }.

journal.cli.social.receive.nothing = du folgst noch keinen Spaces.

journal.cli.social.receive.removed = { $activity_quantity ->
    [one] { $sharer } teilt { $activities } Aktivität nicht mehr, daher ist sie aus deinem Journal verschwunden.
   *[other] { $sharer } teilt { $activities } Aktivitäten nicht mehr, daher sind sie aus deinem Journal verschwunden.
}

journal.cli.social.receive.updated = { $activity_quantity ->
    [one] { $sharer } hat { $activities } Aktivität aktualisiert, die mit dir geteilt wurde.
   *[other] { $sharer } hat { $activities } Aktivitäten aktualisiert, die mit dir geteilt wurden.
}

journal.cli.social.resolve.done = { $account } ist { $did }, gehostet bei { $host }.

journal.cli.social.share.done = "{ $title }" geteilt. sende diese Space-Adresse an die Personen, mit denen du es geteilt hast, damit ihre Journale ihm folgen können:{ $line_break }{ $space }

journal.cli.social.signed.out = du bist nicht angemeldet. beginn mit { $sign_in }

journal.cli.social.signin.declined = die Anmeldung wurde abgelehnt, daher hat sich nichts geändert.

journal.cli.social.signin.done = angemeldet als { $account }.

journal.cli.social.signin.expired = die Anmeldung wurde nicht rechtzeitig abgeschlossen. führ sie erneut aus, um von vorn zu beginnen.

journal.cli.social.signin.open = öffne zum Anmelden diese Adresse in deinem Browser:{ $line_break }{ $address }

journal.cli.social.status.account = angemeldet als { $account } auf { $host }.

journal.cli.social.status.memories = { $memory_quantity ->
    [one] hält { $memories } Aktivität, die mit dir geteilt wurde.
   *[other] hält { $memories } Aktivitäten, die mit dir geteilt wurden.
}

journal.cli.social.status.shares = { $share_quantity ->
    [one] teilt { $shares } Aktivität.
   *[other] teilt { $shares } Aktivitäten.
}

journal.cli.social.status.spaces = { $space_quantity ->
    [one] folgt { $spaces } Space.
   *[other] folgt { $spaces } Spaces.
}

journal.cli.social.sync.current = jede geteilte Aktivität ist auf dem neuesten Stand.

journal.cli.social.sync.removed = { $activity_quantity ->
    [one] { $activities } geteilte Aktivität entfernt, die nicht mehr in deinem Journal ist.
   *[other] { $activities } geteilte Aktivitäten entfernt, die nicht mehr in deinem Journal sind.
}

journal.cli.social.sync.updated = { $activity_quantity ->
    [one] { $activities } geteilte Aktivität aktualisiert.
   *[other] { $activities } geteilte Aktivitäten aktualisiert.
}

journal.cli.social.unshare.done = das Teilen dieser Aktivität wurde beendet, und sie ist aus jedem Space verschwunden, in dem du sie geteilt hast.

journal.cli.social.unshare.unknown = diese Aktivität ist nicht geteilt.

journal.cli.social.usage = Verwendung: { $syntax }{ $line_break }{ $line_break }teile eine Aktivität aus deinem Journal mit Personen, die du auswählst, über einen Space auf deinem eigenen atproto-Konto, und folge Spaces, die andere mit dir teilen. dein Journal hält, was sie teilen, getrennt von deinen eigenen Erinnerungen. nur Entwicklungs-Builds.

journal.cli.speakers.backfill.error.flags.mismatch = Abweichung unveränderlicher Auffüll-Flags für Vorgang { $operation_id }: erwartet commit={ $expected_commit } reattribute={ $expected_reattribute } accumulation={ $expected_accumulation }, erhalten commit={ $got_commit } reattribute={ $got_reattribute } accumulation={ $got_accumulation }

journal.cli.speakers.backfill.error.invalid.flags = ungültige Auffüll-Flags: { $detail }

journal.cli.speakers.backfill.error.journal.root = Zulassung der Journal-Wurzel ist fehlgeschlagen: { $error }

journal.cli.speakers.backfill.error.ledger = Ledger-Vorgang ist fehlgeschlagen: { $error }

journal.cli.speakers.backfill.error.lock = Ausführungssperre des Auffüllens ist fehlgeschlagen: { $error }

journal.cli.speakers.backfill.error.not.found = Auffüll-Vorgang nicht gefunden: { $id }

journal.cli.speakers.backfill.error.scan = Scan des Auffüllens ist fehlgeschlagen: { $error }

journal.cli.speakers.owner.voice.error.identity.invalid = deine eingerichtete Identität ist nicht zugelassen

journal.cli.speakers.owner.voice.error.no.candidate = Kein Kandidat verfügbar

journal.cli.speakers.owner.voice.error.no.saved.voice = Keine Stimme von dir ist gespeichert

journal.cli.speakers.owner.voice.error.stale.candidate = Kandidatenversion stimmt nicht überein oder Generation ist veraltet

journal.cli.speakers.pool.error.centroid.width = Breite des Schwerpunkts im Kandidatenpool stimmt nicht überein

journal.cli.speakers.pool.error.lock = Sperre des Kandidatenpools ist fehlgeschlagen: { $error }

journal.cli.speakers.pool.error.read = Lesen des Kandidatenpools ist fehlgeschlagen: { $error }

journal.cli.speakers.pool.error.write = Schreiben des Kandidatenpools ist fehlgeschlagen: { $error }

journal.cli.speakers.pool.refusal.days.unreadable = Aufholen der Stimmenliste abgelehnt: Journaltage konnten nicht aufgelistet werden

journal.cli.speakers.pool.refusal.format.unsupported = Aufholen der Stimmenliste abgelehnt: die Stimmenliste ist in einem Format, zu dem diese Version nichts hinzufügen kann

journal.cli.speakers.pool.refusal.lock.timeout = Aufholen der Stimmenliste abgelehnt: Zeitüberschreitung der Stimmenlisten-Sperre

journal.cli.speakers.pool.refusal.lock.timeout.kept = Aufholen der Stimmenliste abgelehnt: Zeitüberschreitung der Stimmenlisten-Sperre; was hinzugefügt wurde, wird behalten

journal.cli.speakers.pool.refusal.pool.unreadable = Aufholen der Stimmenliste abgelehnt: Stimmenliste konnte nicht gelesen werden

journal.cli.speakers.pool.refusal.pool.write.failed = Aufholen der Stimmenliste abgelehnt: Stimmenliste konnte nicht geschrieben werden

journal.cli.speakers.pool.refusal.pool.write.failed.kept = Aufholen der Stimmenliste abgelehnt: Stimmenliste konnte nicht geschrieben werden; was hinzugefügt wurde, wird behalten

journal.cli.speakers.pool.refusal.progress.invalid = Aufholen der Stimmenliste abgelehnt: Fortschrittsdatei des Aufholens ist ungültig; das Entfernen von awareness/speaker_candidates_backfill.json aus deinem Journal startet das Aufholen gefahrlos neu

journal.cli.speakers.pool.refusal.progress.unreadable = Aufholen der Stimmenliste abgelehnt: Fortschrittsdatei des Aufholens konnte nicht gelesen werden; prüfe, ob der awareness-Ordner deines Journals gelesen werden kann

journal.cli.speakers.pool.refusal.progress.write.failed = Aufholen der Stimmenliste abgelehnt: Fortschrittsdatei des Aufholens konnte nicht geschrieben werden

journal.cli.speakers.pool.refusal.progress.write.failed.kept = Aufholen der Stimmenliste abgelehnt: Fortschrittsdatei des Aufholens konnte nicht geschrieben werden; was hinzugefügt wurde, wird behalten

journal.cli.stats.arguments = Verwendung: { $syntax }{ $line_break }{ $command }: Fehler: unbekannte Argumente: { $argument }

journal.cli.stats.cause.facet = Facettenspeicher-Fehler: { $detail }

journal.cli.stats.cause.invalid.day = ungültiger Tag { $day }; Kalenderdatum { $date_format } erwartet

journal.cli.stats.cause.io = kein Zugriff auf { $path }: { $detail }

journal.cli.stats.cause.path = Journal-Pfadfehler: { $detail }

journal.cli.stats.cause.read = Journal-Lesefehler: { $detail }

journal.cli.stats.cause.schema = Validierung der Statistiken ist fehlgeschlagen: { $schema } ist { $actual }, erwartet { $expected }

journal.cli.stats.cause.timestamp = Validierung der Statistiken ist fehlgeschlagen: { $timestamp } darf nicht leer sein

journal.cli.stats.cause.validation = Validierung der Statistiken ist fehlgeschlagen: { $detail }

journal.cli.stats.cause.write = Fehler beim atomaren Schreiben des Journals: { $detail }

journal.cli.stats.configuration = Fehler beim Laden der Tageskonfiguration des Journals: { $error }

journal.cli.stats.day.cache = Speichern des Tages-Zwischenspeichers für { $day } ist fehlgeschlagen: { $error }

journal.cli.stats.enumerate = Fehler beim Aufzählen der Journaltage: { $error }

journal.cli.stats.help = Verwendung: { $syntax }{ $line_break }{ $line_break }Ein { $brand }-Journal scannen und Statistiken erzeugen{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $no_cache_option }{ $gap_5 }Zwischenspeichern pro Tag deaktivieren (erneutes Scannen aller Tage erzwingen){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }ausführliche Ausgabe einschalten{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Debug-Protokollierung einschalten

journal.cli.stats.overrides = Validierung der Statistiken ist fehlgeschlagen: Talent-Überschreibungen konnten nicht aus { $path } gelesen werden

journal.cli.stats.publication.durable = veröffentlicht; Dauerhaftigkeit ungewiss: { $source }

journal.cli.stats.publication.observed = veröffentlicht; Beobachtung ungewiss: { $source }

journal.cli.stats.roots = journal-stats ist fehlgeschlagen: { $error }

journal.cli.stats.saved = Statistiken in { $path } gespeichert

journal.cli.stats.tokens.directory = Lesen des Token-Verzeichnisses ist fehlgeschlagen: { $path }

journal.cli.stats.tokens.json = Ungültiger Token { $json_format } in { $path }: { $error }

journal.cli.stats.tokens.load = Laden des Token-Zwischenspeichers für { $path } ist fehlgeschlagen: { $error }

journal.cli.stats.tokens.read = Lesen des Tokens für { $path } ist fehlgeschlagen: { $error }

journal.cli.stats.tokens.save = Speichern des Token-Zwischenspeichers für { $path } ist fehlgeschlagen: { $error }

journal.cli.stats.validate = Fehler beim Validieren von { $filename }: { $error }

journal.cli.stats.write = Fehler beim Schreiben von { $filename }: { $error }

journal.cli.stats.written = { $days ->
    [one] Statistiken für { $days } Tag in { $path } geschrieben
   *[other] Statistiken für { $days } Tage in { $path } geschrieben
}

journal.cli.streams.arguments = Verwendung: { $syntax }{ $break }{ $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.streams.empty = Keine Datenströme gefunden

journal.cli.streams.header.day = Letzter Tag

journal.cli.streams.header.name = Name

journal.cli.streams.header.segment = Letztes Segment

journal.cli.streams.header.sequence = Nr.

journal.cli.streams.header.type = Typ

journal.cli.streams.help = Verwendung: { $syntax }{ $paragraph }Datenstromidentität untersuchen und verwalten{ $paragraph }Positionsargumente:{ $break }{ $indent }{ $name_input }{ $name_space }Datenstromname zum Untersuchen (weglassen, um alle Datenströme aufzulisten){ $paragraph }Optionen:{ $break }{ $indent }{ $help_input }{ $help_space }diese Hilfe anzeigen und beenden{ $break }{ $indent }{ $rebuild_input }{ $rebuild_space }Datenstromzustand aus Markierungen je Segment rekonstruieren{ $break }{ $indent }{ $verbose_input }{ $verbose_space }ausführliche Ausgabe einschalten{ $break }{ $indent }{ $debug_input }{ $debug_space }Debug-Protokollierung einschalten

journal.cli.streams.inspect.malformed = Datenstrom { $path } konnte nicht gelesen werden: fehlerhafter Datensatz

journal.cli.streams.inspect.not.found = Datenstrom nicht gefunden: { $name }

journal.cli.streams.list.error = Datenströme konnten nicht aufgelistet werden: { $error }

journal.cli.streams.marker.sequence = nicht lesbare Markierung { $path }: ungültige Sequenz

journal.cli.streams.marker.unreadable = nicht lesbare Markierung { $path }: Markierung konnte nicht gelesen werden

journal.cli.streams.rebuild.empty = { $segment_quantity ->
    [one] Keine Datenströme gefunden ({ $segments } Segment gescannt)
   *[other] Keine Datenströme gefunden ({ $segments } Segmente gescannt)
}

journal.cli.streams.rebuild.result = { $stream_quantity ->
    [one] Wiederaufbau von { $streams } Datenstrom aus { $segment_quantity ->
        [one] { $segments } Segment:
       *[other] { $segments } Segmente:
    }
   *[other] Wiederaufbau von { $streams } Datenströmen aus { $segment_quantity ->
        [one] { $segments } Segment:
       *[other] { $segments } Segmente:
    }
}

journal.cli.streams.record.malformed = Datenstrom-Datensatz { $path } konnte nicht gelesen werden: fehlerhafter Datensatz

journal.cli.streams.repair.ahead = { $stream }: Sequenz des Datenstromzustands ist den Markierungen voraus; unverändert

journal.cli.streams.repair.locked = { $stream }: { $path } konnte nicht gesperrt werden; unverändert

journal.cli.streams.repair.malformed = { $stream }: fehlerhafter Datenstrom-Datensatz unter { $path }; unverändert

journal.cli.streams.repair.missing = { $stream }: kein Datenstrom-Datensatz unter { $path }; unverändert

journal.cli.streams.repair.write.failed = { $stream }: { $path } konnte nicht geschrieben werden: die atomare Veröffentlichung ist nicht gelungen

journal.cli.streams.segments.error = Segmente konnten nicht aufgelistet werden: { $error }

journal.cli.streams.supervisor = das Journal läuft nicht. starte es mit '{ $command }' und versuch es noch einmal.

journal.cli.talent.compose.facet.missing = Facette '{ $facet }' nicht gefunden

journal.cli.talent.compose.prompt.path.has.no.parent = Talent { $talent }: Prompt-Pfad hat kein übergeordnetes Verzeichnis

journal.cli.talent.compose.prompt.path.is.missing = Talent { $talent }: Prompt-Pfad fehlt

journal.cli.talent.compose.schema.directory.is.unavailable = Talent { $talent }: Schema-Verzeichnis ist nicht verfügbar: { $error }

journal.cli.talent.compose.schema.file.could.not.be.read = Talent { $talent }: Schema-Datei konnte nicht gelesen werden: { $error }

journal.cli.talent.compose.schema.file.is.not.a.valid.json.schema = Talent { $talent }: Schema-Datei ist kein gültiges { $schema_format }: { $path }

journal.cli.talent.compose.schema.file.is.not.valid.json = Talent { $talent }: Schema-Datei ist kein gültiges { $json }: { $path }

journal.cli.talent.compose.schema.file.not.found.cause = Talent { $talent }: Schema-Datei nicht gefunden: { $error }

journal.cli.talent.compose.schema.file.not.found.path = Talent { $talent }: Schema-Datei nicht gefunden: { $path }

journal.cli.talent.compose.schema.must.be.a.string.got = Talent { $talent }: Schema muss eine Zeichenkette sein, erhalten { $type_name }: { $value }

journal.cli.talent.compose.schema.path.escapes.talent.directory = Talent { $talent }: Schema-Pfad verlässt das Talent-Verzeichnis: { $path }

journal.cli.talent.compose.schema.path.must.be.relative = Talent { $talent }: Schema-Pfad muss relativ sein: { $path }

journal.cli.talent.compose.schema.path.must.not.contain = Talent { $talent }: Schema-Pfad darf '..' nicht enthalten: { $path }

journal.cli.talent.compose.settings = die Journal-Konfiguration konnte nicht gelesen werden: { $detail }

journal.cli.talent.config.access.retired = Prompt '{ $talent }' setzt das ausgemusterte Feld { $access_tier }

journal.cli.talent.config.activities = Aktivitätsgeplanter Prompt '{ $talent }' muss eine nicht leere { $activities }-Liste haben (Aktivitätstypen zum Zuordnen, oder { $all_types } für alle Typen).

journal.cli.talent.config.budget.invalid = Prompt '{ $talent }' hat für { $max_output_tokens } den ungültigen Wert { $value }. Erwartet wird eine positive ganze Zahl, gemessen am mitgelieferten Modell.

journal.cli.talent.config.budget.required = Prompt '{ $talent }' fehlt das erforderliche { $max_output_tokens }. Miss seine größte Ausgabe am mitgelieferten Modell und deklariere das 1,5-Fache davon, auf ein Vielfaches von 256 aufgerundet.

journal.cli.talent.config.budget.retired = Prompt '{ $talent }' deklariert das ausgemusterte Feld { $thinking_budget }. Denken wird nur in Denken gesetzt, für das eigene Modell des Eigentümers.

journal.cli.talent.config.cwd.retired = Prompt '{ $talent }' setzt das ausgemusterte Feld { $cwd }

journal.cli.talent.config.frontmatter = das Frontmatter aus { $path } konnte nicht geparst werden

journal.cli.talent.config.output.required = Prompt '{ $talent }' hat type={ $generate }, aber das erforderliche Feld { $output } fehlt.

journal.cli.talent.config.priority = Geplanter Prompt '{ $talent }' fehlt das erforderliche Feld { $priority }. Alle Prompts mit { $schedule } müssen eine ausdrückliche Priorität deklarieren.

journal.cli.talent.config.read = { $path } konnte nicht gelesen werden: { $error }

journal.cli.talent.config.stat = stat für { $path } ist fehlgeschlagen: { $error }

journal.cli.talent.config.stem = Talent-Dateiname hat keinen { $encoding }-Stamm: { $path }

journal.cli.talent.config.type.invalid = Prompt '{ $talent }' hat den ungültigen Typ { $value }. Erwartet wird { $generate }.

journal.cli.talent.config.type.required = Prompt '{ $talent }' hat eine Ausgabe, aber das erforderliche Feld { $type } fehlt.

journal.cli.talent.daily.accepted.response = akzeptierte Antwort fehlt

journal.cli.talent.daily.accepted.result = akzeptiertes Tagesergebnis fehlt

journal.cli.talent.daily.artifact.before = Konflikt: erforderliches Artefakt hat sich nach der Prompt-Vorbereitung geändert

journal.cli.talent.daily.artifact.changed = Konflikt: erforderliche Ausgabe hat sich nach der Vorbereitung geändert

journal.cli.talent.daily.artifact.snapshot = eingefrorenes Abbild des erforderlichen Artefakts fehlt

journal.cli.talent.daily.artifact.snapshot.invalid = ungültiges eingefrorenes Artefakt-Abbild: { $error }

journal.cli.talent.daily.attempt.day = Tag des täglichen Versuchs fehlt

journal.cli.talent.daily.attempt.day.invalid = ungültiger Tag des täglichen Versuchs

journal.cli.talent.daily.attempt.mismatch = { $key } des Tages stimmt nicht überein

journal.cli.talent.daily.attempt.token = Token des täglichen Versuchs fehlt

journal.cli.talent.daily.commit.observer = Zustand der Entitätsfakten erwartet

journal.cli.talent.daily.commit.suggest = Zustand des Entitätsvorschlags erwartet

journal.cli.talent.daily.commit.text = Textausgabe erwartet

journal.cli.talent.daily.facet.changed = Konflikt: besitzende Facette hat sich nach der Prompt-Vorbereitung geändert

journal.cli.talent.daily.facet.identity = eingefrorene Facettenidentität fehlt

journal.cli.talent.daily.facet.path = ungültiger Ausgabepfad der Facette

journal.cli.talent.daily.maintenance.changed = Konflikt: Tageszeit hat sich nach der Prompt-Vorbereitung geändert

journal.cli.talent.daily.maintenance.output = ungültige Wartungsausgabe: { $error }

journal.cli.talent.daily.maintenance.primary = Wartungsausgabe hat keinen Primärwert

journal.cli.talent.daily.maintenance.snapshot = eingefrorenes Tageszeit-Abbild fehlt

journal.cli.talent.daily.observer.changed = Konflikt: Beobachtung hat sich nach der Prompt-Vorbereitung geändert

journal.cli.talent.daily.observer.conflict = Konflikt: { $error }

journal.cli.talent.daily.observer.entity.gone = Konflikt: Beobachtungsentität ist verschwunden

journal.cli.talent.daily.observer.validation = Validierung: { $error }

journal.cli.talent.daily.output.outside = Tagesausgabe liegt außerhalb des Journals

journal.cli.talent.daily.output.path = ungültiger Pfad der Tagesausgabe

journal.cli.talent.daily.owner.alias.claimed = Konflikt: Alias der Hochstufung wurde nach der Vorbereitung beansprucht

journal.cli.talent.daily.owner.alias.target.detached = Konflikt: Alias-Ziel ist nicht mehr verknüpft

journal.cli.talent.daily.owner.identity.blocked.facet = Konflikt: Name der Hochstufung in dieser Facette verborgen

journal.cli.talent.daily.owner.identity.blocked.hidden = Konflikt: die Entität ist verborgen, daher wurden ihre Namen nicht geändert

journal.cli.talent.daily.owner.identity.blocked.identity = Konflikt: Identität der Hochstufung verborgen

journal.cli.talent.daily.owner.identity.changed = Konflikt: hochgestufte Identität hat sich nach der Vorbereitung geändert

journal.cli.talent.daily.owner.identity.deleted = Konflikt: hochgestufte Identität wurde nach der Vorbereitung gelöscht

journal.cli.talent.daily.owner.identity.disappeared = Konflikt: Identität der Hochstufung ist verschwunden

journal.cli.talent.daily.owner.identity.map.group.lost = Konflikt: Hochstufung { $name } passt zu { $directory }, und die Gruppe der Identitätszuordnung davon ist verloren

journal.cli.talent.daily.owner.identity.merged = Konflikt: hochgestufte Identität wurde nach der Vorbereitung durch Zusammenführen entfernt

journal.cli.talent.daily.owner.identity.moved = Konflikt: hochgestufte Identität wurde nach der Vorbereitung verschoben

journal.cli.talent.daily.owner.identity.unreadable = Konflikt: Hochstufung { $name } leitet die Entitäts-ID { $slug } ab, deren Identität { $directory } nicht gelesen werden kann

journal.cli.talent.daily.owner.merge.proposals.changed = Konflikt: Zusammenführungsvorschläge haben sich nach der Vorbereitung geändert

journal.cli.talent.daily.owner.name.matches.multiple = Konflikt: Hochstufung { $name } passt namentlich zu { $count } Identitäten

journal.cli.talent.daily.owner.no.usable.entity.id = Konflikt: Hochstufung { $name } hat keine verwendbare Entitäts-ID

journal.cli.talent.daily.owner.owning.facet.changed.absent = Konflikt: besitzende Facette existiert nicht mehr

journal.cli.talent.daily.owner.owning.facet.changed.invalid = Konflikt: besitzende Facette hat keine gültige stabile Identität

journal.cli.talent.daily.owner.owning.facet.changed.replaced = Konflikt: besitzende Facette wurde nach der Vorbereitung ersetzt

journal.cli.talent.daily.owner.principal.grant.refused.exists = Konflikt: ein Journal-Eigentümer existiert bereits, daher ist diese Person nicht markiert

journal.cli.talent.daily.owner.principal.grant.refused.name = Konflikt: der konfigurierte Name des Eigentümers passt nicht mehr zu dieser Person

journal.cli.talent.daily.owner.principal.settings = die Journal-Einstellungen konnten nicht gelesen werden, daher wurde diese Person nicht als Eigentümer markiert

journal.cli.talent.daily.owner.promoted.entity.blocked.hidden = Konflikt: hochgestufte Entität ist verborgen

journal.cli.talent.daily.owner.promoted.entity.blocked.name = Konflikt: hochgestufte Entität { $name } ist verborgen

journal.cli.talent.daily.owner.relationship.changed = Konflikt: Beziehung der Hochstufung hat sich nach der Vorbereitung geändert

journal.cli.talent.daily.owner.relationship.occupied = Konflikt: Hochstufung { $name } braucht das Beziehungsverzeichnis { $relationship_dir }, und das ist bereits belegt

journal.cli.talent.daily.packet.absent = täglicher Versuch hat kein eingefrorenes Paket

journal.cli.talent.daily.packet.digest = der Digest des Tagespakets stimmt nicht überein

journal.cli.talent.daily.packet.scope = der Umfang des Tagespakets stimmt nicht überein

journal.cli.talent.daily.packet.talent = das Talent des Tagespakets stimmt nicht überein

journal.cli.talent.daily.parse.entities = Entitäten müssen ein Array sein

journal.cli.talent.daily.parse.output = Ausgabe konnte nicht als JSON geparst werden: { $error }

journal.cli.talent.daily.publication.plan = Veröffentlichung erfordert das beibehaltene erzeugte Ergebnis und den genauen vorbereiteten Plan

journal.cli.talent.daily.publication.record = Veröffentlichungsdatensatz fehlt

journal.cli.talent.daily.response.malformed = beibehaltene Tagesantwort ist fehlerhaft

journal.cli.talent.daily.retry.scope = der Umfang des täglichen erneuten Versuchs hat sich geändert

journal.cli.talent.daily.review.aliases = Hochstufung der Prüfung hat keine Aliasse

journal.cli.talent.daily.review.eligibility = eingefrorene Zulässigkeit der Prüfung fehlt

journal.cli.talent.daily.review.hints = eingefrorene Hinweise der Prüfung fehlen

journal.cli.talent.daily.review.inputs = eingefrorene Zulassungseingaben der Prüfung fehlen

journal.cli.talent.daily.review.merges = Ausgabe der Prüfung hat keine Zusammenführungen

journal.cli.talent.daily.review.name = eingefrorener Kandidat hat keinen Namen

journal.cli.talent.daily.review.owner = eingefrorener Eigentümerzustand der Prüfung fehlt

journal.cli.talent.daily.review.promotion = eingefrorener Eigentümerzustand der Hochstufung fehlt

journal.cli.talent.daily.review.promotion.changed = Konflikt: Eigentümerzustand der Hochstufung hat sich nach der Prompt-Vorbereitung geändert

journal.cli.talent.daily.review.promotions = Ausgabe der Prüfung hat keine Hochstufungen

journal.cli.talent.daily.review.proposals = eingefrorener Vorschlagszustand fehlt

journal.cli.talent.daily.review.proposals.changed = Konflikt: Zusammenführungsvorschlag hat sich nach der Prompt-Vorbereitung geändert

journal.cli.talent.daily.review.type = eingefrorener Kandidat hat keinen Typ

journal.cli.talent.daily.type = nicht unterstützter Talent-Typ: { $kind }

journal.cli.talent.daily.validation.aliases = Validierung: hochgestufter Kandidat der Prüfung muss ein Alias-Array enthalten

journal.cli.talent.daily.validation.json = Validierung: ungültiges { $shape }-JSON: { $error }

journal.cli.talent.daily.validation.maintenance.missing = Validierung: Primärwert der Wartung fehlt

journal.cli.talent.daily.validation.maintenance.time = Validierung: Primärwert der Wartung muss HH:MM sein

journal.cli.talent.daily.validation.merges = Validierung: Zusammenführungen der Prüfung müssen ein Array sein

journal.cli.talent.daily.validation.observer = Validierung: entities:entity_observer erfordert ein Entitäten-Array

journal.cli.talent.daily.validation.promotions = Validierung: Hochstufungen der Prüfung müssen ein Array sein

journal.cli.talent.daily.validation.suggest = Validierung: vorgeschlagene Entitäten müssen ein Array sein

journal.cli.talent.daily.write.contract = nicht unterstützter täglicher Schreibvertrag für { $talent }

journal.cli.talent.error.list.choice = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argument { $flag }: ungültige Auswahl: '{ $value }' (wähle aus { $choices })

journal.cli.talent.error.list.unknown = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.talent.error.list.value = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argument { $flag }: genau ein Argument erwartet

journal.cli.talent.error.log.required = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: folgende Argumente sind erforderlich: { $arguments }

journal.cli.talent.error.logs.count = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argument { $count_options }: ungültiger Ganzzahlwert: '{ $value }'

journal.cli.talent.error.logs.value = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argument { $flag }: genau ein Argument erwartet

journal.cli.talent.error.passages.required = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: folgende Argumente sind erforderlich: { $arguments }

journal.cli.talent.error.passages.unknown = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.talent.error.passages.value = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argument { $flag }: genau ein Argument erwartet

journal.cli.talent.error.root.choice = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argument { $subcommand }: ungültige Auswahl: '{ $value }' (wähle aus { $choices })

journal.cli.talent.error.root.encoding = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argumente sind nicht gültig als { $encoding }

journal.cli.talent.error.root.unknown = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.talent.error.show.required = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: folgende Argumente sind erforderlich: { $arguments }

journal.cli.talent.error.show.unknown = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.talent.error.show.value = Verwendung: { $syntax }{ $line_break }{ $program }: Fehler: Argument { $flag }: genau ein Argument erwartet

journal.cli.talent.facet.malformed = fehlerhafte Daten in { $path }

journal.cli.talent.facet.malformed.line = fehlerhafte Daten in { $path } in Zeile { $line }

journal.cli.talent.facet.object = die Facetten-Deklaration ist kein Objekt: { $path }

journal.cli.talent.floor.invalid.budget = { $budget } fehlt oder ist ungültig

journal.cli.talent.floor.mismatch = { $entries ->
        [one] Anzahl der Einträge stimmt nicht überein: { $entries } Eintrag geparst, aber { $sources ->
                [one] { $sources } Eintragsquelle vorhanden
               *[other] { $sources } Eintragsquellen vorhanden
            }
       *[other] Anzahl der Einträge stimmt nicht überein: { $entries } Einträge geparst, aber { $sources ->
                [one] { $sources } Eintragsquelle vorhanden
               *[other] { $sources } Eintragsquellen vorhanden
            }
    }

journal.cli.talent.help.list = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $syntax_continuation_5 }{ $line_break }                        Nach Zeitplantyp filtern{ $line_break }{ $syntax_continuation_7 }{ $line_break }                        Nach Ursprung filtern{ $line_break }{ $indent_2 }{ $disabled_option }{ $gap_12 }Deaktivierte Prompts einschließen{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Als { $jsonl } ausgeben

journal.cli.talent.help.log = Verwendung: { $syntax }{ $line_break }{ $line_break }Positionsargumente:{ $line_break }  { $id }Agent-ID{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }Rohes { $jsonl } ausgeben{ $line_break }{ $indent_2 }{ $full_option }{ $gap_6 }Ereignisdetails erweitern

journal.cli.talent.help.logs = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }Positionsargumente:{ $line_break }  { $agent }Auf einen bestimmten Agenten filtern{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $syntax_continuation_9 }{ $line_break }                        Anzahl anzuzeigender Läufe (Standard: 20){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Nur Läufe dieses Tages anzeigen{ $line_break }{ $indent_2 }{ $daily_option }{ $gap_15 }Nur täglich geplante Läufe anzeigen{ $line_break }{ $indent_2 }{ $errors_option }{ $gap_14 }Nur Fehlerläufe anzeigen{ $line_break }{ $indent_2 }{ $summary_option }{ $gap_13 }Gruppierte Zusammenfassung anzeigen

journal.cli.talent.help.passages = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }                           Name{ $line_break }{ $line_break }Genaue Passagen aus der Quelle kopieren, die ein Aktivitätstalent liest.{ $line_break }Eine kurze Quelleneinheit wird vollständig behalten, ohne einen Aufruf des Modells; eine längere{ $line_break }braucht eine Vervollständigung, um auszuwählen, was kopiert wird. Keine Passagen werden in{ $line_break }das Journal geschrieben; die Nutzung einer Vervollständigung wird wie jede andere protokolliert.{ $line_break }{ $line_break }Positionsargumente:{ $line_break }  { $name }Name des Aktivitätstalents, etwa work oder conversation{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Das Passagen-Dokument als { $json } ausgeben{ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Tag der Aktivität{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Facette der Aktivität{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }Aktivitäts-ID

journal.cli.talent.help.root = Verwendung: { $syntax }{ $line_break }{ $line_break }Talent-Prompt-Konfigurationen prüfen{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $list }Prompts nach Zeitplan gruppiert auflisten{ $line_break }    { $show }Details für einen bestimmten Prompt anzeigen{ $line_break }    { $passages }Quellpassagen aus der Quelle einer Aktivität kopieren{ $line_break }    { $logs }Protokoll der letzten Talent-Läufe anzeigen{ $line_break }    { $log }Ereignisse eines Agentenlaufs anzeigen{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }ausführliche Ausgabe einschalten{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Debug-Protokollierung einschalten

journal.cli.talent.help.show = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                           Name{ $line_break }{ $line_break }Positionsargumente:{ $line_break }  { $name }Prompt-Name{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Als { $jsonl } ausgeben{ $line_break }{ $indent_2 }{ $prompt_option }{ $gap_14 }Vollständigen Prompt-Kontext anzeigen (Probelauf-Modus){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Tag für den Prompt-Kontext{ $line_break }{ $indent_2 }{ $segment_option }{ $gap_2 }Segment für segmentgeplante Prompts{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Facette für Prompts mit mehreren Facetten{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }Aktivitäts-ID für aktivitätsgeplante Prompts{ $line_break }{ $indent_2 }{ $query_option }{ $gap_10 }Beispielabfrage für Tool-Agenten{ $line_break }{ $indent_2 }{ $full_option }{ $gap_16 }Vollständigen Inhalt ohne Kürzung anzeigen

journal.cli.talent.hook.briefing.date.overflow = Datumsüberlauf des Briefings

journal.cli.talent.hook.briefing.failed = der Vor-Hook des Morgen-Briefings ist fehlgeschlagen: { $error }

journal.cli.talent.hook.budget = die vorbereitete Anfrage passt nicht in das Eingabe-Budget der Untergrenze

journal.cli.talent.hook.daily.lookback = Rückblick des Tageszeitplans außerhalb von { $range }

journal.cli.talent.hook.day.invalid = ungültiger oder nicht auswertbarer Tag: '{ $day }'

journal.cli.talent.hook.day.missing = fehlender Tag

journal.cli.talent.hook.document.stream = kein Dokumentimport-Segment

journal.cli.talent.hook.entity.minimum = das Entitätspaket für { $entity } hat mindestens { $count } Zeichen; das Maximum ist { $maximum }

journal.cli.talent.hook.entity.name = fehlender Entitätsname

journal.cli.talent.hook.entity.size = das Entitätspaket für { $entity } hat { $count } Zeichen; das Maximum ist { $maximum }

journal.cli.talent.hook.facet.missing = fehlende Facette

journal.cli.talent.hook.facet.read = Facette { $facet } konnte nicht gelesen werden: { $error }

journal.cli.talent.hook.facets.list = Facetten konnten nicht aufgelistet werden: { $error }

journal.cli.talent.hook.facts.size = der Faktenkontext der Entität hat { $count } Zeichen; das Maximum ist { $maximum }

journal.cli.talent.hook.failure = { $phase }-Hook '{ $stage }' für Talent '{ $talent }': { $detail }

journal.cli.talent.hook.identities.read = Entitätsidentitäten konnten nicht gelesen werden: { $error }

journal.cli.talent.hook.identity.object = die Entitätsidentität ist kein Objekt: { $path }

journal.cli.talent.hook.lookup.control = ungültige Pfadkomponente '{ $candidate }': der Name enthält ein Steuerzeichen

journal.cli.talent.hook.lookup.dotcomponent = ungültige Pfadkomponente '{ $candidate }': { $dot } und { $parent } sind keine gültigen Namen

journal.cli.talent.hook.lookup.empty = ungültige Pfadkomponente '{ $candidate }': der Name ist leer

journal.cli.talent.hook.lookup.kind = { $path } ist kein Verzeichnis ({ $kind })

journal.cli.talent.hook.lookup.layout = direktes Layout erfordert den Datenstrom "{ $default_stream }", erhalten { $stream }

journal.cli.talent.hook.lookup.rootorprefix = ungültige Pfadkomponente '{ $candidate }': der Name muss eine einzelne relative Komponente sein

journal.cli.talent.hook.lookup.separator = ungültige Pfadkomponente '{ $candidate }': der Name enthält ein Pfadtrennzeichen

journal.cli.talent.hook.possible.no.prompt = das Talent hat keinen Prompt

journal.cli.talent.hook.resolution.absent = die aufgelöste Mehrdeutigkeit { $ambiguity } nennt die Entität { $entity }, die in diesem Auflösungsaufruf fehlt

journal.cli.talent.hook.resolution.hidden = die aufgelöste Mehrdeutigkeit { $ambiguity } nennt die ausgeblendete Entität { $entity }

journal.cli.talent.hook.review.changed = die Prüfungseingaben haben sich während der Vorbereitung geändert

journal.cli.talent.hook.stream.layout = direktes Layout erfordert den Datenstrom "{ $default_stream }", erhalten "{ $stream }"

journal.cli.talent.hook.suggest.size = der Vorschlagskontext der Entität hat { $count } Zeichen; das Maximum ist { $maximum }

journal.cli.talent.hook.suggestions.array = im Vorschlags-JSON fehlt das { $entities }-Array

journal.cli.talent.hook.suggestions.json = ungültiges Vorschlags-JSON in { $path }: { $error }

journal.cli.talent.hook.suggestions.missing = fehlendes Vorschlagsartefakt: { $path }

journal.cli.talent.hook.sunday = der Anfragetag { $day } ist kein Sonntag

journal.cli.talent.hook.transcript.bytes = die Transkript-Bytes sind kein gültiges UTF-8

journal.cli.talent.hook.transcript.read = Transkript { $file } konnte nicht gelesen werden: { $error }

journal.cli.talent.hook.transcript.utf8 = Transkript { $file } ist kein UTF-8: { $error }

journal.cli.talent.hook.week.incomplete = die Woche mit Ende { $end } ist vor heute { $today } noch nicht abgeschlossen

journal.cli.talent.last.age = vor { $age }

journal.cli.talent.last.age.duration = vor { $age } ({ $duration })

journal.cli.talent.list.empty = Keine Prompts gefunden, die zu den Filtern passen.

journal.cli.talent.list.group.activity = Aktivität:

journal.cli.talent.list.group.cadence = Kadenz:

journal.cli.talent.list.group.daily = täglich:

journal.cli.talent.list.group.segment = Segment:

journal.cli.talent.list.group.unscheduled = ohne Zeitplan:

journal.cli.talent.list.group.weekly = wöchentlich:

journal.cli.talent.list.header.last = LETZTER LAUF

journal.cli.talent.list.header.name = NAME

journal.cli.talent.list.header.tags = ETIKETTEN

journal.cli.talent.list.header.title = TITEL

journal.cli.talent.list.tag.disabled = deaktiviert

journal.cli.talent.list.tag.fail = NICHT GELUNGEN

journal.cli.talent.list.visible = { $count ->
    [one] { $count } Prompt ({ $hidden } deaktiviert und ausgeblendet, { $disabled_flag } verwenden)
   *[other] { $count } Prompts ({ $hidden } deaktiviert und ausgeblendet, { $disabled_flag } verwenden)
    }

journal.cli.talent.log.event.thinking = Denken

journal.cli.talent.log.event.tool = Werkzeug

journal.cli.talent.log.event.updated = aktualisiert

journal.cli.talent.log.missing = Talent-Lauf nicht gefunden: { $run }

journal.cli.talent.log.tokens = { $result } [{ $input } ein/{ $output } aus]

journal.cli.talent.logs.date = { $month } { $day_time }

journal.cli.talent.logs.day.invalid = Ungültiges Format für { $day_flag }: { $day }. Erwartet { $day_format }.

journal.cli.talent.logs.total = gesamt

journal.cli.talent.outcome.facet.write = FacetEntityWriteError: { $error }

journal.cli.talent.outcome.object = das Ergebnis ist kein JSON-Objekt

journal.cli.talent.outcome.observation.write = ObservationWriteError: { $error }

journal.cli.talent.outcome.observer.entities = entities ist keine Liste

journal.cli.talent.outcome.observer.parse = das Ergebnis konnte nicht als JSON gelesen werden

journal.cli.talent.outcome.review.arrays = im Ergebnis fehlen die erwarteten Arrays

journal.cli.talent.outcome.review.candidate = EntityReviewCandidateError: { $error }

journal.cli.talent.outcome.review.parse = ungültiges JSON

journal.cli.talent.passages.generate = Quellpassagen konnten generate nicht erreichen: { $error }

journal.cli.talent.passages.generate.missing = Quellpassagen konnten generate nicht erreichen: fehlende benachbarte Programmdatei { $path }

journal.cli.talent.passages.generate.parent = Quellpassagen konnten generate nicht erreichen: die aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.talent.passages.incomplete = einige Quelleneinheiten haben keine Passagen; sieh dir das Ergebnis jeder Einheit an

journal.cli.talent.passages.unavailable = Quellpassagen nicht verfügbar: { $detail }

journal.cli.talent.passages.unit.failed = # Einheit { $index } · { $kind } · keine Passagen: { $reason }

journal.cli.talent.passages.unit.failed.time = # Einheit { $index } · { $kind } · { $time } · keine Passagen: { $reason }

journal.cli.talent.passages.unit.not.attempted = # Einheit { $index } · { $kind } · nicht versucht

journal.cli.talent.passages.unit.not.attempted.time = # Einheit { $index } · { $kind } · { $time } · nicht versucht

journal.cli.talent.passages.unit.passages = { $count ->
    [one] # Einheit { $index } · { $kind } · { $count } Passage
   *[other] # Einheit { $index } · { $kind } · { $count } Passagen
    }

journal.cli.talent.passages.unit.passages.time = { $count ->
    [one] # Einheit { $index } · { $kind } · { $time } · { $count } Passage
   *[other] # Einheit { $index } · { $kind } · { $time } · { $count } Passagen
    }

journal.cli.talent.passages.verbatim = Wortgetreu aus der Quelle:

journal.cli.talent.path.absolute = der Journal-Pfad muss relativ sein

journal.cli.talent.path.backslash = der Journal-Pfad muss Schrägstriche / verwenden

journal.cli.talent.path.component = der Journal-Pfad enthält eine ungültige Komponente

journal.cli.talent.path.day = der Tag muss { $day_format } sein

journal.cli.talent.path.empty = der Journal-Pfad darf nicht leer sein

journal.cli.talent.path.escape = { $relative } verlässt die Journal-Wurzel (aufgelöst zu { $path })

journal.cli.talent.path.unreadable = der Journal-Pfad ist nicht lesbar

journal.cli.talent.prepare.configuration.name.declares.an.invalid.mobile.local.timeout = die Konfiguration '{ $name }' deklariert { $timeout } als ungültig

journal.cli.talent.prepare.invalid.config.must.have.type.day.or.prompt = Ungültige Konfiguration: { $type }, { $day } oder { $prompt } muss vorhanden sein

journal.cli.talent.prepare.invalid.config.segment.or.span.requires.day = Ungültige Konfiguration: { $segment } oder { $span } erfordert { $day }

journal.cli.talent.prepare.invalid.talent.type.kind = ungültiger Talent-Typ '{ $kind }'

journal.cli.talent.prepare.memory.incomplete = privater Erinnerungskontext ist unvollständig

journal.cli.talent.prepare.request.field.field.is.retired = das Anfragefeld '{ $field }' ist ausgemustert

journal.cli.talent.prepare.request.overrides.field.for.talent.name.are.not.allowed = Die Überschreibungen der Anfrage '{ $field }' für Talent '{ $name }' sind nicht erlaubt ({ $declared } != { $requested })

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed = die Überschreibungen der Anfrage für '{ $field }' sind nicht erlaubt; Anbieter und Modell des Denkens werden nur in Denken konfiguriert

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed.budget = die Überschreibungen der Anfrage für '{ $field }' sind nicht erlaubt; die Budgets eines Talents sind seine eigenen

journal.cli.talent.prepare.segment.missing = { $count ->
    [one] Segmentverzeichnis nicht gefunden: { $segments }
   *[other] Segmentverzeichnisse nicht gefunden: { $segments }
    }

journal.cli.talent.prepare.talent.name.declares.field.in.frontmatter = das Talent '{ $name }' deklariert '{ $field }' im Frontmatter; Anbieter und Modell des Denkens werden nur in Denken konfiguriert

journal.cli.talent.prepare.talent.name.declares.outbound.approval.in.frontmatter = das Talent '{ $name }' deklariert { $outbound_approval } im Frontmatter; dieses Feld gibt es nur in der Startkonfiguration und es darf nicht aus einer Talent-Definition kommen

journal.cli.talent.prepare.talent.name.not.found = Talent '{ $name }' nicht gefunden

journal.cli.talent.prepare.talent.request.missing.name = der Talent-Anfrage fehlt der Name

journal.cli.talent.preview.all.lines = (verwende { $full_flag }, um alle { $count } Zeilen zu sehen)

journal.cli.talent.preview.banner = Vorschau dessen, was dieses Talent senden würde. Dieselbe Zusammenstellung, die ein echter Lauf verwendet; das Modell wird nicht aufgerufen.

journal.cli.talent.preview.effective = Effektiver Prompt für: { $talent }

journal.cli.talent.preview.empty = (leer)

journal.cli.talent.preview.instruction = ANWEISUNG

journal.cli.talent.preview.no.day = Kein Tag angegeben, daher enthält diese Vorschau keine Aufnahmen eines Tages. Übergib { $day_flag } { $day_format }, um sie einzuschließen.

journal.cli.talent.preview.omitted = { $count ->
    [one] ... ({ $count } Zeile ausgelassen)
   *[other] ... ({ $count } Zeilen ausgelassen)
    }

journal.cli.talent.preview.omitted.legacy = ... ({ $count } Zeilen ausgelassen)

journal.cli.talent.preview.prestep = Die Prompt-Vorschau kann dieses Talent nicht zusammenstellen: eine erforderliche Vorstufe ist nicht verfügbar.

journal.cli.talent.preview.recovery.choose.a.completed.owner.activity.not.an.anticipated.or.cogi = Wähle eine abgeschlossene Aktivität der Person, nicht einen anticipated- oder cogitate-Datensatz.

journal.cli.talent.preview.recovery.choose.a.talent.configured.for.this.activity.kind = Wähle ein Talent, das für diese Aktivitätsart konfiguriert ist.

journal.cli.talent.preview.recovery.choose.a.talent.that.loads.its.activity.s.sources = Wähle ein Talent, das die Quellen seiner Aktivität lädt.

journal.cli.talent.preview.recovery.choose.a.talent.that.reads.its.activity.source.without.a.pre = Wähle ein Talent, das die Quelle seiner Aktivität ohne Vorstufe liest.

journal.cli.talent.preview.recovery.choose.a.talent.whose.required.pre.step.is.available.in.the = Wähle ein Talent, dessen erforderliche Vorstufe in der nativen Laufzeitumgebung verfügbar ist.

journal.cli.talent.preview.recovery.choose.a.talent.whose.schedule.is.activity = Wähle ein Talent, dessen Zeitplan { $schedule } ist

journal.cli.talent.preview.recovery.choose.an.activity.id.stored.in.the.requested.day.and.facet = Wähle eine Aktivitäts-ID, die im angeforderten Tag und in der angeforderten Facette gespeichert ist.

journal.cli.talent.preview.recovery.choose.an.activity.with.at.least.one.recorded.segment = Wähle eine Aktivität mit mindestens einem aufgezeichneten Segment.

journal.cli.talent.preview.recovery.choose.either.activity.or.segment.not.both = Wähle entweder { $activity_flag } oder { $segment_flag }, nicht beides.

journal.cli.talent.preview.recovery.correct.the.activity.preview.input.then.retry.detail = Korrigiere die Eingabe der Aktivitätsvorschau und versuch es dann noch einmal: { $detail }

journal.cli.talent.preview.recovery.correct.the.talent.configuration.detail = Korrigiere die Talent-Konfiguration: { $detail }

journal.cli.talent.preview.recovery.make.the.selected.activity.and.talent.eligible.then.retry = Mache die ausgewählte Aktivität und das ausgewählte Talent zulässig und versuch es dann noch einmal.

journal.cli.talent.preview.recovery.make.the.selected.talent.eligible.then.retry = Mache das ausgewählte Talent zulässig und versuch es dann noch einmal.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.facet.name.and.activity.id = Übergib { $day_flag } { $yyyymmdd }, { $facet_flag } { $name } und { $activity_flag } ID.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.with.activity = Übergib { $day_flag } { $yyyymmdd } mit { $activity_flag }.

journal.cli.talent.preview.recovery.pass.facet.name.with.activity = Übergib { $facet_flag } { $name } mit { $activity_flag }.

journal.cli.talent.preview.recovery.remove.duplicate.records.with.this.id.in.the.requested.day.a = Entferne doppelte Datensätze mit dieser ID im angeforderten Tag und in der angeforderten Facette.

journal.cli.talent.preview.recovery.repair.the.activity.span.so.every.segment.is.a.non.empty.str = Repariere die Aktivitätsspanne, sodass jedes Segment eine nicht leere Zeichenkette ist.

journal.cli.talent.preview.recovery.restore.a.readable.activity.record.file.then.retry.detail = Stelle eine lesbare Datei des Aktivitätsdatensatzes wieder her und versuch es dann noch einmal: { $detail }

journal.cli.talent.preview.recovery.restore.a.readable.segment.directory.then.retry.detail = Stelle ein lesbares Segmentverzeichnis wieder her und versuch es dann noch einmal: { $detail }

journal.cli.talent.preview.recovery.restore.readable.segment.directories.then.retry.detail = Stelle lesbare Segmentverzeichnisse wieder her und versuch es dann noch einmal: { $detail }

journal.cli.talent.preview.recovery.restore.the.selected.activity.segment.then.retry = Stelle das ausgewählte Aktivitätssegment wieder her und versuch es dann noch einmal.

journal.cli.talent.preview.skipped = Dieses Talent würde nicht laufen: { $reason }

journal.cli.talent.show.body = Inhalt:

journal.cli.talent.show.body.lines = { $count ->
    [one] { $count } Zeile
   *[other] { $count } Zeilen
    }

journal.cli.talent.show.hook = Prompt '{ $talent }' ist ein Hook-Prompt und kann nicht direkt ausgeführt werden.

journal.cli.talent.show.missing = Prompt nicht gefunden: { $talent }

journal.cli.talent.show.missing.path = Prompt nicht gefunden: { $talent }{ $line_break }  nachgesehen: { $path }

journal.cli.talent.show.variables = Variablen:

journal.cli.talent.startup.executable = das Talent ist fehlgeschlagen: die aktuelle Programmdatei konnte nicht untersucht werden: { $error }

journal.cli.talent.startup.package = das Talent ist fehlgeschlagen: das installierte Paket { $package } konnte nicht gefunden werden

journal.cli.talent.work.duration = ungültige Work-Quelldauer: { $error }

journal.cli.talent.work.duration.range = Work-Quelldauer liegt außerhalb des Bereichs

journal.cli.talent.work.interval = ungültiges Work-Quellintervall: { $error }

journal.cli.talent.work.interval.range = Work-Quellintervall liegt außerhalb des Bereichs

journal.cli.talent.work.key = ungültiges Work-Quellsegment: { $segment }

journal.cli.talent.work.read = die Work-Segmentergebnisse konnten nicht gelesen werden: { $error }

journal.cli.talent.work.unavailable = Work-Quellsegment ist nicht verfügbar: { $day }/{ $segment }

journal.cli.top.crashed.attempts = { $name } (Versuche: { $attempts })

journal.cli.top.crashed.crashed = { $name } (Abgestürzt; Versuche: { $attempts })

journal.cli.top.crashed.heading.attention = Dienste, die Aufmerksamkeit brauchen:

journal.cli.top.crashed.heading.crashed = Abgestürzt:

journal.cli.top.crashed.retrying = { $name } (Wird erneut versucht; Versuche: { $attempts })

journal.cli.top.error.apply = das Anwenden des Terminals ist fehlgeschlagen: { $detail }

journal.cli.top.error.journal.config = Journal-Konfiguration: { $detail }

journal.cli.top.error.screen = der Eintritt in den Terminalbildschirm ist fehlgeschlagen: { $detail }

journal.cli.top.error.stdin.not.tty = stdin ist kein Terminal

journal.cli.top.error.stdout.not.tty = stdout ist kein Terminal

journal.cli.top.error.terminal = Terminalfehler: { $detail }

journal.cli.top.error.transport = Transportfehler: { $detail }

journal.cli.top.error.width = die Abfrage der Terminalbreite ist fehlgeschlagen: { $detail }

journal.cli.top.error.width.unavailable = Terminalbreite nicht verfügbar

journal.cli.top.footer.empty = q: Beenden

journal.cli.top.footer.multi = ↑/↓: Navigieren  r: Neu starten  q: Beenden

journal.cli.top.footer.single = r: Neu starten  q: Beenden

journal.cli.top.header.last.log = Letztes Protokoll

journal.cli.top.header.mb = MB

journal.cli.top.header.percent = %

journal.cli.top.header.pid = PID

journal.cli.top.header.runtime = Laufzeit

journal.cli.top.header.service = Dienst

journal.cli.top.header.task = Aufgabe

journal.cli.top.header.uptime = Betriebszeit

journal.cli.top.malformed.events = fehlerhafte Ereignisse: { $count } ({ $reason })

journal.cli.top.observe.describe = beschreiben

journal.cli.top.observe.idle = [LEERLAUF]

journal.cli.top.observe.idle.locked = [LEERLAUF] gesperrt

journal.cli.top.observe.live.badge = [LIVE]

journal.cli.top.observe.recent = Kürzlich: { $segments }

journal.cli.top.observe.screencast = Bildschirmaufnahme { $elapsed }

journal.cli.top.observe.tmux.badge = [TMUX]

journal.cli.top.observe.tmux.captures = { $quantity ->
    [one] { $count } Bildschirmbild
    *[other] { $count } Bildschirmbilder
    }

journal.cli.top.observe.transcribe = transkribieren

journal.cli.top.observe.voice = Stimme { $hits }

journal.cli.top.queued = in der Warteschlange: { $commands }

journal.cli.top.reconnecting = (wird neu verbunden)

journal.cli.top.section.observe = Beobachtung

journal.cli.top.section.think = Denken

journal.cli.top.section.thinking.health = Zustand des Denkens

journal.cli.top.task.status.failed = nicht gelungen

journal.cli.top.task.status.gone = verschwunden

journal.cli.top.task.status.ok = ok

journal.cli.top.think.agents = { $total_quantity ->
    [one] { $completed }/{ $total } Lauf
    *[other] { $completed }/{ $total } Läufe
    }

journal.cli.top.think.last.failed = { $failed } nicht gelungen

journal.cli.top.think.last.prefix = Zuletzt: { $success } ok

journal.cli.top.think.segments = Seg { $completed }/{ $total }

journal.cli.top.thinking.checking = (wird geprüft)

journal.cli.top.thinking.unavailable = (Status nicht verfügbar)

journal.cli.top.title = solstone-Aktivitätsmanager

journal.cli.top.unknown = unbekannt

journal.cli.top.waiting.services = (wartet auf Dienste)

journal.cli.top.waiting.status = (wartet auf den Status)

journal.cli.top.waiting.think = (wartet auf Denken)
