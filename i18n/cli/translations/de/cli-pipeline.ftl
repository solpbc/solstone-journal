journal.cli.backup.config.canonical.recovery.characters = kanonischer Wiederherstellungsschlüssel enthält ungültige { $Crockford }-Zeichen

journal.cli.backup.config.canonical.recovery.length = kanonischer Wiederherstellungsschlüssel muss genau 64 Zeichen lang sein

journal.cli.backup.config.entropy = Sicherungsschlüssel konnte nicht erzeugt werden.

journal.cli.backup.config.hosted.delete = gehostete Bindung konnte nicht gelöscht werden.

journal.cli.backup.config.hosted.read = gehostete Bindung konnte nicht gelesen werden.

journal.cli.backup.config.hosted.write = gehostete Bindung konnte nicht gespeichert werden.

journal.cli.backup.config.invalid.destination.shape = Sicherungsziel muss ein { $JSON }-Objekt sein

journal.cli.backup.config.invalid.mode = Sicherungsmodus muss { $byo } oder { $operated } sein

journal.cli.backup.config.invalid.offload.bytes = Byte-Werte der Sicherungsauslagerung müssen positive ganze Zahlen oder null sein

journal.cli.backup.config.invalid.offload.enabled = Sicherungsauslagerung aktiviert muss ein boolescher Wert sein

journal.cli.backup.config.invalid.offload.shape = Sicherungsauslagerung muss { $enabled }, { $budget_bytes }, { $floor_bytes } enthalten

journal.cli.backup.config.invalid.offload.status = Status der Sicherungsauslagerung muss { $ok_value }, { $skipped_value }, { $stalled_value } oder { $error_value } sein

journal.cli.backup.config.invalid.restore.counters = Zähler der Sicherungswiederherstellung müssen nichtnegative ganze Zahlen sein

journal.cli.backup.config.invalid.restore.scope = Umfang der Sicherungswiederherstellung muss { $day_value }, { $all_value } oder { $journal_value } sein

journal.cli.backup.config.invalid.restore.status = Status der Sicherungswiederherstellung muss { $ok_value }, { $no_op_value }, { $refused_value }, { $degraded_value } oder { $error_value } sein

journal.cli.backup.config.invalid.retention.shape = Aufbewahrung der Sicherung muss { $hourly }, { $daily }, { $weekly }, { $monthly } enthalten

journal.cli.backup.config.invalid.retention.value = Aufbewahrungswerte der Sicherung müssen nichtnegative ganze Zahlen sein

journal.cli.backup.config.invalid.verification.status = Prüfstatus der Sicherung muss { $ok_value }, { $skipped_value } oder { $error_value } sein

journal.cli.backup.config.lock.timeout = Sperre für { $path } konnte nicht innerhalb von { $seconds } s erlangt werden

journal.cli.backup.config.missing.credential = fehlende Sicherungszugangsdaten: { $key }

journal.cli.backup.config.recovery.parse = Wiederherstellungsschlüssel muss nach dem Bereinigen genau 64 { $Crockford }-Zeichen enthalten

journal.cli.backup.config.stored.keys = Sicherungsschlüssel müssen Zeichenketten sein, wenn sie vorhanden sind

journal.cli.backup.config.teardown.recovery.corrupt = Wiederherstellungsdatensatz des Abbaus ist beschädigt oder ungültig.

journal.cli.backup.config.teardown.recovery.delete = Wiederherstellungsdatensatz des Abbaus konnte nicht gelöscht werden.

journal.cli.backup.config.teardown.recovery.read = Wiederherstellungsdatensatz des Abbaus konnte nicht gelesen werden.

journal.cli.backup.config.teardown.recovery.write = Wiederherstellungsdatensatz des Abbaus konnte nicht gespeichert werden.

journal.cli.backup.config.unsupported.backend = nicht unterstütztes Sicherungs-Backend

journal.cli.backup.destination.backend.prefix = Das Repository muss mit { $backend }: beginnen (zum Beispiel { $example }).

journal.cli.backup.destination.hosted = { $company } betreibt deinen Sicherungsspeicher, hier gibt es also kein Ziel zum Ändern. verwalte ihn im Diensteportal.

journal.cli.backup.destination.refused.auth.failed = das Ziel hat die Zugangsdaten abgelehnt. nichts wurde gespeichert.

journal.cli.backup.destination.refused.bucket.not.found = diese Zugangsdaten können keinen Bucket mit diesem Namen öffnen. prüfe den Bucket-Namen im Repository. nichts wurde gespeichert.

journal.cli.backup.destination.refused.locked = die Sicherung dort ist beschäftigt. versuch es gleich noch einmal. nichts wurde gespeichert.

journal.cli.backup.destination.refused.other = das Ziel konnte nicht geöffnet werden. prüfe das Repository und die Zugangsdaten, dann versuch es noch einmal. nichts wurde gespeichert.

journal.cli.backup.destination.refused.repo.missing = keine Sicherung wurde dort mit diesen Zugangsdaten gefunden. nichts wurde gespeichert.

journal.cli.backup.destination.refused.repository.key.refused = es gibt dort bereits eine Sicherung, die dein Wiederherstellungsschlüssel nicht öffnen kann. wähle ein anderes Ziel. nichts wurde gespeichert.

journal.cli.backup.destination.refused.timeout = das Ziel hat nicht rechtzeitig geantwortet. prüfe das Repository und die Zugangsdaten, dann versuch es noch einmal. nichts wurde gespeichert.

journal.cli.backup.destination.setup.failed = eine Sicherung konnte dort nicht eingerichtet werden ({ $reason }). nichts wurde gespeichert.

journal.cli.backup.destination.switch.confirm = zum Wechseln führ es erneut mit { $confirm_new_repository_flag } aus.

journal.cli.backup.destination.switch.lead = das ist ein anderes Repository. deine bestehende Sicherung bleibt, wo sie ist, unberührt, und dein Journal sichert nicht dorthin und stellt nicht daraus wieder her.

journal.cli.backup.enable.complete = Sicherung aktiviert.

journal.cli.backup.enable.complete.key = Sicherung aktiviert. Dein Wiederherstellungsschlüssel:{ $line_break }{ $key_grid }

journal.cli.backup.enable.confirm.key = Dein Wiederherstellungsschlüssel (schreib ihn auf - er ist der einzige Weg zum Wiederherstellen):{ $line_break }{ $key_grid }{ $line_break }Bestätige, indem du den Schlüssel zurückleitest: { $command_enable }

journal.cli.backup.enable.destination.required = Lege zuerst ein Ziel fest: { $command_destination_set }

journal.cli.backup.enable.key.mismatch = Wiederherstellungsschlüssel stimmte nicht überein.

journal.cli.backup.enable.repository.missing = Repository nicht gefunden; initialisiere es vorher oder setze { $recovery_setting } in der Konfiguration.

journal.cli.backup.error = Fehler: { $message }

journal.cli.backup.help.destination = Aufruf: { $syntax }

journal.cli.backup.help.offload = Aufruf: { $syntax }

journal.cli.backup.help.recovery.key = Aufruf: { $syntax }

journal.cli.backup.help.root = Aufruf: { $syntax }

journal.cli.backup.hosted.field = { $name } fehlt.

journal.cli.backup.init.code = Initialisierung des Repository ist nicht gelungen ({ $code_field }{ $returncode }).

journal.cli.backup.init.failed = Initialisierung des Repository ist nicht gelungen.

journal.cli.backup.init.retained = Initialisierung des Repository ist nicht gelungen; zurückgehaltener Betriebszustand erfordert eine Prüfung.

journal.cli.backup.input.io = { $stdin_stream } I/O-Fehler.

journal.cli.backup.input.json = ungültiges { $json_format } auf { $stdin_stream }: { $error }

journal.cli.backup.input.limit = { $stdin_stream }-Anfrage überschreitet das { $json_format }-Eingabelimit.

journal.cli.backup.input.object = erwartetes { $json_format }-Objekt auf { $stdin_stream }.

journal.cli.backup.offload.media.byte.overflow = Überlauf der Medienbyte-Summe

journal.cli.backup.offload.media.file.overflow = Überlauf der Mediendatei-Summe

journal.cli.backup.offload.preview.held = eine schreibgeschützte Vorschau der Auslagerung ist nicht verfügbar. keine Auslagerungsarbeit wurde gestartet.

journal.cli.backup.offload.restore.day.invalid = Ungültiger Tag.

journal.cli.backup.offload.restore.day.or.all = Verwende entweder einen Tag oder { $all_flag }, nicht beides.

journal.cli.backup.offload.restore.day.required = Gib einen Tag oder { $all_flag } an.

journal.cli.backup.offload.restore.heading = Wiederherstellung der Sicherungsauslagerung:

journal.cli.backup.offload.restore.reason.archive.unbound = der Sicherungsdatensatz nennt das Archiv nicht, das zum Wiederherstellen dieser Dateien nötig ist

journal.cli.backup.offload.restore.reason.backup.not.ready = deine Sicherung ist nicht eingerichtet, die ausgelagerten Dateien können also nicht zurückgeholt werden

journal.cli.backup.offload.restore.reason.destination.admission.failed = das Sicherungsziel konnte nicht erreicht werden

journal.cli.backup.offload.restore.reason.failed = der Wiederherstellungsversuch ist aus einem nicht genannten Grund nicht gelungen

journal.cli.backup.offload.restore.reason.insufficient.free.space = auf diesem Computer ist nicht genug freier Speicherplatz, um die Dateien zurückzuholen

journal.cli.backup.offload.restore.reason.ledger.degraded = der Datensatz über das, was ausgelagert wurde, ist beschädigt und kann nicht zum Wiederherstellen verwendet werden

journal.cli.backup.offload.restore.reason.legacy.read.failed = die Sicherungskopie von Medien, die eine frühere Version deines Journals ausgelagert hat, konnte nicht gelesen werden und wurde deshalb nicht wiederhergestellt

journal.cli.backup.offload.restore.reason.legacy.review.held = Medien, die eine frühere Version deines Journals ausgelagert hat, konnten nicht gegen deine Sicherung geprüft werden und wurden deshalb nicht wiederhergestellt

journal.cli.backup.offload.restore.reason.legacy.verification.failed = die Sicherungskopie von Medien, die eine frühere Version deines Journals ausgelagert hat, stimmte nicht mit ihrem Datensatz überein und wurde deshalb nicht wiederhergestellt

journal.cli.backup.offload.restore.reason.locked = die Sicherung am Ziel ist gesperrt, meist durch einen Lauf, der noch läuft, oder einen, der unterbrochen wurde

journal.cli.backup.offload.restore.reason.missing.file.after.restore = eine Datei, die mit der Wiederherstellung hätte zurückkommen sollen, fehlt

journal.cli.backup.offload.restore.reason.nothing.to.restore = es gab nichts Ausgelagertes zum Wiederherstellen

journal.cli.backup.offload.restore.reason.rclone.unavailable = das Programm { $rclone_program } ist nicht verfügbar

journal.cli.backup.offload.restore.reason.repo.missing = es gibt am Ziel noch keine Sicherung

journal.cli.backup.offload.restore.reason.restic.unavailable = das Sicherungsprogramm { $restic_program } ist nicht verfügbar

journal.cli.backup.offload.restore.reason.restore.conflict = eine andere Datei liegt bereits dort, wo mindestens eine dieser Dateien hingehört, also wurde die bereits vorhandene Datei behalten und die Sicherungskopie nicht wiederhergestellt

journal.cli.backup.offload.restore.reason.restore.held = das Wiederherstellen ausgelagerter Dateien ist vorübergehend nicht verfügbar

journal.cli.backup.offload.restore.reason.restore.not.published = mindestens eine Datei konnte diesmal nicht in dein Journal zurückgebracht werden, und an ihre Stelle kam nichts, also versuch es später noch einmal

journal.cli.backup.offload.restore.reason.segment.identity = eine der Aufnahmen deines Journals konnte nicht gegen ihren eigenen Identitätsdatensatz überprüft werden

journal.cli.backup.offload.restore.reason.segment.missing = der Ort im Journal, zu dem diese Dateien gehören, existiert auf diesem Computer nicht mehr

journal.cli.backup.offload.restore.reason.stream.marker.failed = die Dateien wurden wiederhergestellt, aber die anschließende Aktualisierung der Datensätze des Journals ist nicht gelungen

journal.cli.backup.offload.restore.reason.timeout = es lief zu lange und hat aufgehört

journal.cli.backup.offload.restore.reason.unknown.code = ohne Grund aufgehört ({ $code })

journal.cli.backup.offload.restore.reason.verification.failed = eine wiederhergestellte Datei stimmte nicht mit dem überein, was sie sein sollte und ist deshalb möglicherweise beschädigt

journal.cli.backup.offload.restore.unfinished = die Wiederherstellung wurde nicht sauber abgeschlossen, also versuch es später noch einmal.

journal.cli.backup.offload.status.heading = Sicherungsauslagerung:

journal.cli.backup.offload.status.invalid = ungültiger Auslagerungsstatus.

journal.cli.backup.offload.status.overflow = Überlauf der Medienstatus-Summe

journal.cli.backup.offload.use.installed = führ `{ $command_offload_run }` mit dem installierten Befehl { $program } aus.

journal.cli.backup.output.json = { $json_format }-Ausgabe konnte nicht dargestellt werden.

journal.cli.backup.pause.already = Sicherung ist bereits pausiert.

journal.cli.backup.pause.complete = Sicherung pausiert. führ `{ $command_resume }` aus, um sie wieder zu starten.

journal.cli.backup.pause.off = Sicherung ist aus, es gibt also nichts zu pausieren.

journal.cli.backup.payload.backend = Backend fehlt.

journal.cli.backup.payload.credential = Zugangsdaten fehlen: { $key }.

journal.cli.backup.payload.credentials = Zugangsdaten fehlen.

journal.cli.backup.payload.recovery.key = { $key_field } fehlt.

journal.cli.backup.payload.repository = Repository fehlt.

journal.cli.backup.payload.unsupported = Nicht unterstütztes Backend.

journal.cli.backup.prune.complete = Ausdünnen der Aufbewahrung abgeschlossen.

journal.cli.backup.prune.detail.failure.rclone.unavailable = Ausdünnen ist nicht gelungen: das Programm { $rclone_program } ist nicht verfügbar. { $detail_field }{ $detail }

journal.cli.backup.prune.detail.failure.restic.unavailable = Ausdünnen ist nicht gelungen: das Sicherungsprogramm { $restic_program } ist nicht verfügbar. { $detail_field }{ $detail }

journal.cli.backup.prune.failure.access = Ausdünnen ist nicht gelungen: das Ziel hat die Berechtigung für diesen Schreibvorgang verweigert.

journal.cli.backup.prune.failure.auth.failed = Ausdünnen ist nicht gelungen: das Ziel hat die Zugangsdaten abgelehnt.

journal.cli.backup.prune.failure.binding.invalid = Ausdünnen ist nicht gelungen: die Sicherungseinstellungen deines Journals sind nicht mehr gültig.

journal.cli.backup.prune.failure.binding.superseded = Ausdünnen ist nicht gelungen: neuere Sicherungseinstellungen haben diese ersetzt.

journal.cli.backup.prune.failure.broker.error = Ausdünnen ist nicht gelungen: der Sicherungsdienst, den { $company } betreibt, konnte die Anfrage nicht abschließen.

journal.cli.backup.prune.failure.broker.unreachable = Ausdünnen ist nicht gelungen: der Sicherungsdienst, den { $company } betreibt, konnte nicht erreicht werden.

journal.cli.backup.prune.failure.bucket.not.found = Ausdünnen ist nicht gelungen: deine Zugangsdaten können den Bucket nicht öffnen, den das Repository nennt.

journal.cli.backup.prune.failure.failed = Ausdünnen ist nicht gelungen: es wurde kein Grund genannt.

journal.cli.backup.prune.failure.hosted.entitlement.inactive = Ausdünnen ist nicht gelungen: { $company } betreibt keine Sicherung für dich.

journal.cli.backup.prune.failure.journal.path.unresolved = Ausdünnen ist nicht gelungen: dein Journal konnte nicht gefunden werden.

journal.cli.backup.prune.failure.locked = Ausdünnen ist nicht gelungen: die Sicherung am Ziel ist gesperrt, meist durch einen Lauf, der noch läuft, oder einen, der unterbrochen wurde.

journal.cli.backup.prune.failure.rclone.unavailable = Ausdünnen ist nicht gelungen: das Programm { $rclone_program } ist nicht verfügbar.

journal.cli.backup.prune.failure.repo.exists = Ausdünnen ist nicht gelungen: es gibt dort bereits eine Sicherung.

journal.cli.backup.prune.failure.repo.missing = Ausdünnen ist nicht gelungen: es gibt am Ziel noch keine Sicherung.

journal.cli.backup.prune.failure.repository.unsupported = Ausdünnen ist nicht gelungen: das Repository in deinen Sicherungseinstellungen beginnt nicht mit { $s3_backend } oder { $b2_backend }, um zu seinem Backend zu passen.

journal.cli.backup.prune.failure.restic.unavailable = Ausdünnen ist nicht gelungen: das Sicherungsprogramm { $restic_program } ist nicht verfügbar.

journal.cli.backup.prune.failure.timeout = Ausdünnen ist nicht gelungen: es lief zu lange und hat aufgehört.

journal.cli.backup.prune.failure.unknown.code = Ausdünnen ist nicht gelungen: ohne Grund aufgehört ({ $code }).

journal.cli.backup.prune.failure.unreachable = Ausdünnen ist nicht gelungen: das Ziel konnte nicht erreicht werden.

journal.cli.backup.prune.skipped = Ausdünnen übersprungen (nicht aktiviert oder nicht konfiguriert).

journal.cli.backup.recovery.key.missing = Kein Wiederherstellungsschlüssel ist festgelegt.

journal.cli.backup.restore.already.set.up = Wiederherstellung konnte nicht starten: dein Journal ist bereits eingerichtet, darüber kann also nicht wiederhergestellt werden. um eine Sicherung zu verwenden, starte ein neues Journal und wähl während der Ersteinrichtung Wiederherstellen.

journal.cli.backup.restore.complete = Wiederherstellung abgeschlossen: { $counters }, { $integrity_field }{ $integrity }, { $resume_field }{ $resumable }.

journal.cli.backup.restore.degraded.integrity.failed = Wiederherstellung mit einem Problem abgeschlossen: { $counters }; die Sicherung hat die Integritätsprüfung nicht bestanden und ist möglicherweise beschädigt, dein Journal ist also nicht auf die Sicherung gesetzt ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.integrity.unverified = Wiederherstellung mit einem Problem abgeschlossen: { $counters }; die Integritätsprüfung konnte nicht laufen (die Sicherung war beschäftigt oder es kam zu einer Zeitüberschreitung), dein Journal ist also nicht auf die Sicherung gesetzt ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.other = Wiederherstellung mit einem Problem abgeschlossen: { $counters }; Wiederherstellung mit beeinträchtigtem Nachweis abgeschlossen ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.restore.summary.missing = Wiederherstellung mit einem Problem abgeschlossen: { $counters }; die Zusammenfassung der Wiederherstellung hat gefehlt oder war fehlerhaft ({ $reason_field }{ $code }).

journal.cli.backup.restore.failed.counters = Wiederherstellung ist nicht gelungen: { $counters }.

journal.cli.backup.restore.failure.body.rebuild.failed = Wiederherstellung ist nicht gelungen: dein Journal konnte nicht aus den wiederhergestellten Dateien neu aufgebaut werden; { $counters }.

journal.cli.backup.restore.failure.destination.admission.failed = Wiederherstellung ist nicht gelungen: das Sicherungsziel konnte nicht erreicht werden; { $counters }.

journal.cli.backup.restore.failure.destination.invalid = Wiederherstellung ist nicht gelungen: das Sicherungsziel ist nicht gültig; { $counters }.

journal.cli.backup.restore.failure.destination.publish.failed = Wiederherstellung ist nicht gelungen: das wiederhergestellte Journal konnte nicht an seinem Ziel gespeichert werden; { $counters }.

journal.cli.backup.restore.failure.full.scan.failed = Wiederherstellung ist nicht gelungen: ein vollständiger Scan des wiederhergestellten Journals ist nicht gelungen; { $counters }.

journal.cli.backup.restore.failure.insufficient.free.space = Wiederherstellung ist nicht gelungen: es gibt nicht genug freien Speicherplatz für diese Wiederherstellung, es hat sich also nichts geändert; { $counters }.

journal.cli.backup.restore.failure.integrity.check.io.failed = Wiederherstellung ist nicht gelungen: die wiederhergestellten Dateien konnten nicht geprüft werden; { $counters }.

journal.cli.backup.restore.failure.integrity.failed = Wiederherstellung ist nicht gelungen: die Sicherung hat die Integritätsprüfung nicht bestanden und ist möglicherweise beschädigt, dein Journal ist also nicht auf die Sicherung gesetzt; { $counters }.

journal.cli.backup.restore.failure.integrity.unverified = Wiederherstellung ist nicht gelungen: die Integritätsprüfung konnte nicht laufen (die Sicherung war beschäftigt oder es kam zu einer Zeitüberschreitung), dein Journal ist also nicht auf die Sicherung gesetzt; { $counters }.

journal.cli.backup.restore.failure.invalid.key = Wiederherstellung ist nicht gelungen: der eingegebene Wiederherstellungsschlüssel stimmt nicht mit dieser Sicherung überein; { $counters }.

journal.cli.backup.restore.failure.journal.already.set.up = Wiederherstellung ist nicht gelungen: dein Journal ist bereits eingerichtet, darüber kann also nicht wiederhergestellt werden. um eine Sicherung zu verwenden, starte ein neues Journal und wähl während der Ersteinrichtung Wiederherstellen; { $counters }.

journal.cli.backup.restore.failure.journal.snapshot.not.found = Wiederherstellung ist nicht gelungen: es wurde keine Sicherung deines Journals gefunden; { $counters }.

journal.cli.backup.restore.failure.recovery.confirmation.publish.failed = Wiederherstellung ist nicht gelungen: die Wiederherstellungsbestätigung konnte nicht gespeichert werden; { $counters }.

journal.cli.backup.restore.failure.recovery.key.publish.failed = Wiederherstellung ist nicht gelungen: der Wiederherstellungsschlüssel konnte nicht gespeichert werden; { $counters }.

journal.cli.backup.restore.failure.restic.unavailable = Wiederherstellung ist nicht gelungen: das Sicherungsprogramm { $restic_program } ist nicht verfügbar; { $counters }.

journal.cli.backup.restore.failure.restore.failed = Wiederherstellung ist nicht gelungen: die Wiederherstellung wurde nicht abgeschlossen; { $counters }.

journal.cli.backup.restore.failure.restore.io.failed = Wiederherstellung ist nicht gelungen: Dateien konnten während der Wiederherstellung nicht geschrieben werden; { $counters }.

journal.cli.backup.restore.failure.restore.record.failed = Wiederherstellung ist nicht gelungen: das Ergebnis der Wiederherstellung konnte nicht festgehalten werden; { $counters }.

journal.cli.backup.restore.failure.restore.summary.missing = Wiederherstellung ist nicht gelungen: die Wiederherstellung wurde abgeschlossen, aber ihre Zusammenfassung fehlt; { $counters }.

journal.cli.backup.restore.failure.snapshot.catalog.invalid = Wiederherstellung ist nicht gelungen: die Liste der Sicherungen ist ungültig; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.failed = Wiederherstellung ist nicht gelungen: die Liste der Sicherungen konnte nicht abgerufen werden; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.io.failed = Wiederherstellung ist nicht gelungen: die Liste der Sicherungen konnte nicht gelesen werden; { $counters }.

journal.cli.backup.restore.failure.snapshot.selection.ambiguous = Wiederherstellung ist nicht gelungen: mehr als eine Sicherung hat gepasst und keine konnte automatisch gewählt werden; { $counters }.

journal.cli.backup.restore.failure.unknown.code = Wiederherstellung ist nicht gelungen: ohne Grund aufgehört ({ $code }); { $counters }.

journal.cli.backup.restore.recording.failure.body.rebuild.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: dein Journal konnte nicht aus den wiederhergestellten Dateien neu aufgebaut werden.

journal.cli.backup.restore.recording.failure.destination.admission.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: das Sicherungsziel konnte nicht erreicht werden.

journal.cli.backup.restore.recording.failure.destination.invalid = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: das Sicherungsziel ist nicht gültig.

journal.cli.backup.restore.recording.failure.destination.publish.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: das wiederhergestellte Journal konnte nicht an seinem Ziel gespeichert werden.

journal.cli.backup.restore.recording.failure.full.scan.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: ein vollständiger Scan des wiederhergestellten Journals ist nicht gelungen.

journal.cli.backup.restore.recording.failure.insufficient.free.space = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: es gibt nicht genug freien Speicherplatz für diese Wiederherstellung, es hat sich also nichts geändert.

journal.cli.backup.restore.recording.failure.integrity.check.io.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die wiederhergestellten Dateien konnten nicht geprüft werden.

journal.cli.backup.restore.recording.failure.integrity.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Sicherung hat die Integritätsprüfung nicht bestanden und ist möglicherweise beschädigt, dein Journal ist also nicht auf die Sicherung gesetzt.

journal.cli.backup.restore.recording.failure.integrity.unverified = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Integritätsprüfung konnte nicht laufen (die Sicherung war beschäftigt oder es kam zu einer Zeitüberschreitung), dein Journal ist also nicht auf die Sicherung gesetzt.

journal.cli.backup.restore.recording.failure.invalid.key = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: der eingegebene Wiederherstellungsschlüssel stimmt nicht mit dieser Sicherung überein.

journal.cli.backup.restore.recording.failure.journal.already.set.up = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: dein Journal ist bereits eingerichtet, darüber kann also nicht wiederhergestellt werden. um eine Sicherung zu verwenden, starte ein neues Journal und wähl während der Ersteinrichtung Wiederherstellen.

journal.cli.backup.restore.recording.failure.journal.snapshot.not.found = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: es wurde keine Sicherung deines Journals gefunden.

journal.cli.backup.restore.recording.failure.recovery.confirmation.publish.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Wiederherstellungsbestätigung konnte nicht gespeichert werden.

journal.cli.backup.restore.recording.failure.recovery.key.publish.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: der Wiederherstellungsschlüssel konnte nicht gespeichert werden.

journal.cli.backup.restore.recording.failure.restic.unavailable = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: das Sicherungsprogramm { $restic_program } ist nicht verfügbar.

journal.cli.backup.restore.recording.failure.restore.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Wiederherstellung wurde nicht abgeschlossen.

journal.cli.backup.restore.recording.failure.restore.io.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: Dateien konnten während der Wiederherstellung nicht geschrieben werden.

journal.cli.backup.restore.recording.failure.restore.record.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: das Ergebnis der Wiederherstellung konnte nicht festgehalten werden.

journal.cli.backup.restore.recording.failure.restore.summary.missing = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Wiederherstellung wurde abgeschlossen, aber ihre Zusammenfassung fehlt.

journal.cli.backup.restore.recording.failure.snapshot.catalog.invalid = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Liste der Sicherungen ist ungültig.

journal.cli.backup.restore.recording.failure.snapshot.list.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Liste der Sicherungen konnte nicht abgerufen werden.

journal.cli.backup.restore.recording.failure.snapshot.list.io.failed = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: die Liste der Sicherungen konnte nicht gelesen werden.

journal.cli.backup.restore.recording.failure.snapshot.selection.ambiguous = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: mehr als eine Sicherung hat gepasst und keine konnte automatisch gewählt werden.

journal.cli.backup.restore.recording.failure.unknown.code = Festhalten des Ergebnisses ist ebenfalls nicht gelungen: ohne Grund aufgehört ({ $code }).

journal.cli.backup.restore.unrecorded = Wiederherstellung abgeschlossen, aber das Festhalten des Ergebnisses ist nicht gelungen: { $counters } ({ $reason_field }{ $restore_record }).

journal.cli.backup.resume.complete = Sicherung fortgesetzt. die nächste Sicherung läuft nach ihrem üblichen Zeitplan.

journal.cli.backup.resume.not.paused = Sicherung ist nicht pausiert.

journal.cli.backup.rotation.failed = Auswechslung ist nicht gelungen: nicht gelungen.

journal.cli.backup.rotation.failure.access = Auswechslung ist nicht gelungen: das Ziel hat die Berechtigung für diesen Schreibvorgang verweigert.

journal.cli.backup.rotation.failure.auth.failed = Auswechslung ist nicht gelungen: das Ziel hat die Zugangsdaten abgelehnt.

journal.cli.backup.rotation.failure.binding.invalid = Auswechslung ist nicht gelungen: die Sicherungseinstellungen deines Journals sind nicht mehr gültig.

journal.cli.backup.rotation.failure.binding.superseded = Auswechslung ist nicht gelungen: neuere Sicherungseinstellungen haben diese ersetzt.

journal.cli.backup.rotation.failure.broker.error = Auswechslung ist nicht gelungen: der Sicherungsdienst, den { $company } betreibt, konnte die Anfrage nicht abschließen.

journal.cli.backup.rotation.failure.broker.unreachable = Auswechslung ist nicht gelungen: der Sicherungsdienst, den { $company } betreibt, konnte nicht erreicht werden.

journal.cli.backup.rotation.failure.bucket.not.found = Auswechslung ist nicht gelungen: deine Zugangsdaten können den Bucket nicht öffnen, den das Repository nennt.

journal.cli.backup.rotation.failure.failed = Auswechslung ist nicht gelungen: es wurde kein Grund genannt.

journal.cli.backup.rotation.failure.hosted.entitlement.inactive = Auswechslung ist nicht gelungen: { $company } betreibt keine Sicherung für dich.

journal.cli.backup.rotation.failure.journal.path.unresolved = Auswechslung ist nicht gelungen: dein Journal konnte nicht gefunden werden.

journal.cli.backup.rotation.failure.locked = Auswechslung ist nicht gelungen: die Sicherung am Ziel ist gesperrt, meist durch einen Lauf, der noch läuft, oder einen, der unterbrochen wurde.

journal.cli.backup.rotation.failure.rclone.unavailable = Auswechslung ist nicht gelungen: das Programm { $rclone_program } ist nicht verfügbar.

journal.cli.backup.rotation.failure.repo.exists = Auswechslung ist nicht gelungen: es gibt dort bereits eine Sicherung.

journal.cli.backup.rotation.failure.repo.missing = Auswechslung ist nicht gelungen: es gibt am Ziel noch keine Sicherung.

journal.cli.backup.rotation.failure.repository.unsupported = Auswechslung ist nicht gelungen: das Repository in deinen Sicherungseinstellungen beginnt nicht mit { $s3_backend } oder { $b2_backend }, um zu seinem Backend zu passen.

journal.cli.backup.rotation.failure.restic.unavailable = Auswechslung ist nicht gelungen: das Sicherungsprogramm { $restic_program } ist nicht verfügbar.

journal.cli.backup.rotation.failure.timeout = Auswechslung ist nicht gelungen: es lief zu lange und hat aufgehört.

journal.cli.backup.rotation.failure.unknown.code = Auswechslung ist nicht gelungen: ohne Grund aufgehört ({ $code }).

journal.cli.backup.rotation.failure.unreachable = Auswechslung ist nicht gelungen: das Ziel konnte nicht erreicht werden.

journal.cli.backup.rotation.key = Neuer Wiederherstellungsschlüssel (schreib ihn auf):{ $line_break }{ $key_grid }

journal.cli.backup.rotation.skipped = Auswechslung des Wiederherstellungsschlüssels übersprungen (Sicherung nicht konfiguriert).

journal.cli.backup.run.complete = Sicherung abgeschlossen (Abbild { $snapshot }).

journal.cli.backup.run.failure.access = Sicherung ist nicht gelungen: das Ziel hat die Berechtigung für diesen Schreibvorgang verweigert.

journal.cli.backup.run.failure.auth.failed = Sicherung ist nicht gelungen: das Ziel hat die Zugangsdaten abgelehnt.

journal.cli.backup.run.failure.binding.invalid = Sicherung ist nicht gelungen: die Sicherungseinstellungen deines Journals sind nicht mehr gültig.

journal.cli.backup.run.failure.binding.superseded = Sicherung ist nicht gelungen: neuere Sicherungseinstellungen haben diese ersetzt.

journal.cli.backup.run.failure.broker.error = Sicherung ist nicht gelungen: der Sicherungsdienst, den { $company } betreibt, konnte die Anfrage nicht abschließen.

journal.cli.backup.run.failure.broker.unreachable = Sicherung ist nicht gelungen: der Sicherungsdienst, den { $company } betreibt, konnte nicht erreicht werden.

journal.cli.backup.run.failure.bucket.not.found = Sicherung ist nicht gelungen: deine Zugangsdaten können den Bucket nicht öffnen, den das Repository nennt.

journal.cli.backup.run.failure.failed = Sicherung ist nicht gelungen: es wurde kein Grund genannt.

journal.cli.backup.run.failure.hosted.entitlement.inactive = Sicherung ist nicht gelungen: { $company } betreibt keine Sicherung für dich.

journal.cli.backup.run.failure.journal.path.unresolved = Sicherung ist nicht gelungen: dein Journal konnte nicht gefunden werden.

journal.cli.backup.run.failure.locked = Sicherung ist nicht gelungen: die Sicherung am Ziel ist gesperrt, meist durch einen Lauf, der noch läuft, oder einen, der unterbrochen wurde.

journal.cli.backup.run.failure.rclone.unavailable = Sicherung ist nicht gelungen: das Programm { $rclone_program } ist nicht verfügbar.

journal.cli.backup.run.failure.repo.exists = Sicherung ist nicht gelungen: es gibt dort bereits eine Sicherung.

journal.cli.backup.run.failure.repo.missing = Sicherung ist nicht gelungen: es gibt am Ziel noch keine Sicherung.

journal.cli.backup.run.failure.repository.unsupported = Sicherung ist nicht gelungen: das Repository in deinen Sicherungseinstellungen beginnt nicht mit { $s3_backend } oder { $b2_backend }, um zu seinem Backend zu passen.

journal.cli.backup.run.failure.restic.unavailable = Sicherung ist nicht gelungen: das Sicherungsprogramm { $restic_program } ist nicht verfügbar.

journal.cli.backup.run.failure.timeout = Sicherung ist nicht gelungen: es lief zu lange und hat aufgehört.

journal.cli.backup.run.failure.unknown.code = Sicherung ist nicht gelungen: ohne Grund aufgehört ({ $code }).

journal.cli.backup.run.failure.unreachable = Sicherung ist nicht gelungen: das Ziel konnte nicht erreicht werden.

journal.cli.backup.run.partial.snapshot.count = { $quantity ->
    [one] Sicherung unvollständig (Abbild { $snapshot }). 1 Datei konnte nicht gelesen werden und ist deshalb nicht darin. Führ `{ $command }` erneut aus, sobald das behoben ist.
   *[other] Sicherung unvollständig (Abbild { $snapshot }). { $count } Dateien konnten nicht gelesen werden und sind deshalb nicht darin. Führ `{ $command }` erneut aus, sobald das behoben ist.
}

journal.cli.backup.run.partial.snapshot.count.reason = { $quantity ->
    [one] Sicherung unvollständig (Abbild { $snapshot }). 1 Datei konnte nicht gelesen werden und ist deshalb nicht darin. Die erste: { $reason }. Führ `{ $command }` erneut aus, sobald das behoben ist.
   *[other] Sicherung unvollständig (Abbild { $snapshot }). { $count } Dateien konnten nicht gelesen werden und sind deshalb nicht darin. Die erste: { $reason }. Führ `{ $command }` erneut aus, sobald das behoben ist.
}

journal.cli.backup.run.partial.snapshot.unmeasured = Sicherung unvollständig (Abbild { $snapshot }). einige Dateien konnten nicht gelesen werden und sind deshalb nicht darin. Führ `{ $command_run }` erneut aus, sobald das behoben ist.

journal.cli.backup.run.partial.unconfirmed.count = { $quantity ->
    [one] Sicherung unvollständig. 1 Datei konnte nicht gelesen werden und ist deshalb nicht darin. und das Abbild konnte nicht bestätigt werden. Führ `{ $command }` erneut aus, sobald das behoben ist.
   *[other] Sicherung unvollständig. { $count } Dateien konnten nicht gelesen werden und sind deshalb nicht darin. und das Abbild konnte nicht bestätigt werden. Führ `{ $command }` erneut aus, sobald das behoben ist.
}

journal.cli.backup.run.partial.unconfirmed.count.reason = { $quantity ->
    [one] Sicherung unvollständig. 1 Datei konnte nicht gelesen werden und ist deshalb nicht darin. und das Abbild konnte nicht bestätigt werden. Die erste: { $reason }. Führ `{ $command }` erneut aus, sobald das behoben ist.
   *[other] Sicherung unvollständig. { $count } Dateien konnten nicht gelesen werden und sind deshalb nicht darin. und das Abbild konnte nicht bestätigt werden. Die erste: { $reason }. Führ `{ $command }` erneut aus, sobald das behoben ist.
}

journal.cli.backup.run.partial.unconfirmed.unmeasured = Sicherung unvollständig. einige Dateien konnten nicht gelesen werden und sind deshalb nicht darin. und das Abbild konnte nicht bestätigt werden. Führ `{ $command_run }` erneut aus, sobald das behoben ist.

journal.cli.backup.run.skipped = Sicherung übersprungen (nicht aktiviert oder nicht konfiguriert).

journal.cli.backup.teardown.backup.only.held = deine Sicherung hält die einzige Kopie mancher deiner Medien, es wurden also keine Sicherungsänderungen vorgenommen. stelle zuerst auf der Sicherungsseite alles wieder her, dann versuch es noch einmal.

journal.cli.backup.teardown.confirm = Abbau der Sicherung wird ohne { $yes_flag } abgelehnt. Das vergisst alle Abbilder.

journal.cli.backup.teardown.not.started = das Ausschalten und Löschen deiner Sicherung konnte nicht gestartet werden. es wurden keine Sicherungsänderungen vorgenommen.

journal.cli.backup.teardown.partial = Abbau der Sicherung wurde nicht abgeschlossen. behalte die Wiederherstellungsdateien und prüfe die festgehaltenen Auswirkungen, bevor du es erneut versuchst.

journal.cli.backup.teardown.prelaunch.held = das Ausschalten und Löschen deiner Sicherung ist hier nicht verfügbar. es wurden keine Sicherungsänderungen vorgenommen.

journal.cli.backup.usage.destination = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.backup.usage.missing.destination = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $required }

journal.cli.backup.usage.missing.offload = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $required }

journal.cli.backup.usage.missing.recovery.key = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $required }

journal.cli.backup.usage.missing.root = { $command }: Fehler: die folgenden Argumente sind erforderlich: { $required }

journal.cli.backup.usage.offload = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.backup.usage.recovery.key = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.backup.usage.root = { $command }: Fehler: unbekannte Argumente: { $arguments }

journal.cli.client.date.short = { $weekday } { $month } { $day }

journal.cli.client.date.short.year = { $weekday } { $month } { $day } '{ $year }

journal.cli.client.erase.backups.conflict = Fehler: wähle { $from_backups } oder { $skip_backups }, nicht beides.

journal.cli.client.erase.backups.missing = Fehler: die Sicherung ist nicht eingerichtet, also gibt es keine Sicherungen, aus denen es entfernt werden kann.

journal.cli.client.erase.backups.next = Wird als Nächstes aus deinen Sicherungen entfernt.

journal.cli.client.erase.backups.resume = Wird aus deinen Sicherungen entfernt, sobald du die Sicherung fortsetzt.

journal.cli.client.erase.backups.unavailable = Fehler: dieses Journal kann noch nichts aus deinen Sicherungen entfernen.

journal.cli.client.erase.backups.unchecked = Fehler: dein Journal konnte deine Sicherungen gerade eben nicht prüfen. Versuch es noch einmal, oder übergib { $skip_backups }, um endgültig zu löschen, ohne es aus deinen Sicherungen zu entfernen.

journal.cli.client.error.detail = Fehler: { $detail }

journal.cli.client.error.malformed = die Antwort des Journals konnte nicht gelesen werden.

journal.cli.client.error.timeout = Das Journal hat nicht rechtzeitig geantwortet.

journal.cli.client.error.unreachable = dein Journal war über { $http } nicht erreichbar.

journal.cli.client.error.unreadable = Das Journal hat einen unlesbaren Fehler zurückgegeben.

journal.cli.client.lock.acquire = die sidecar-Sperre konnte nicht erlangt werden: { $error }

journal.cli.client.lock.open = die Sperrdatei kann nicht geöffnet werden: { $error }

journal.cli.client.missing.argument = Fehler: fehlendes Argument { $argument }

journal.cli.client.network.cli.pair.join.hint = kopple dieses Gerät mit:

journal.cli.client.network.cli.pair.local.network.closed = die Kopplung kann nicht gestartet werden. dein Journal ist für Geräte in deinem Netzwerk geschlossen. führ `{ $call_open }` aus, um ein weiteres Gerät über dein Netzwerk zu koppeln, oder schalte dein privates Netzwerk ein, um über das Relay zu koppeln.

journal.cli.client.network.cli.pair.no.lan.address = die Kopplung kann nicht gestartet werden. dein Journal ist noch unter keiner Netzwerkadresse erreichbar. schalte dein privates Netzwerk ein, um von überall zu koppeln, oder verbinde dieses Gerät mit deinem Netzwerk zu Hause.

journal.cli.client.network.clients.empty = Keine autorisierten Clients.

journal.cli.client.network.clients.seen = { $fingerprint }  { $label }  zuletzt gesehen { $when }

journal.cli.client.network.device.fingerprint.absent = Kein gekoppeltes Gerät mit Fingerabdruck { $fingerprint }

journal.cli.client.network.device.label.absent = Kein gekoppeltes Gerät mit Beschriftung { $label }

journal.cli.client.network.device.label.cleared = Gerätebeschriftung gesetzt auf: (geleert)

journal.cli.client.network.device.label.set = Gerätebeschriftung gesetzt auf: { $label }

journal.cli.client.network.devices.empty = Noch keine Geräte gekoppelt.

journal.cli.client.network.devices.seen = - { $label } · hinzugefügt { $added } · zuletzt gesehen { $seen } [{ $fingerprint }]

journal.cli.client.network.devices.total = Gekoppelte Geräte: { $total }

journal.cli.client.network.journal.id = Journal-ID:     { $id }

journal.cli.client.network.label.options = Fehler: gib entweder { $label } oder { $clear } an

journal.cli.client.network.linked.systems.heading = Gekoppelte Systeme:

journal.cli.client.network.listen.state = { $protocol } Zustand: (frage { $convey } { $path } für den Live-Zustand ab)

journal.cli.client.network.local.network.open = dein Journal ist für Geräte in deinem Netzwerk offen. Geräte im selben Netzwerk können direkt mit deinem Journal koppeln.

journal.cli.client.network.local.network.this.pc = dein Journal ist für Geräte in deinem Netzwerk geschlossen. die { $brand }-App auf diesem Computer koppelt weiterhin, und Geräte über das Relay auch, sobald dein privates Netzwerk an ist.

journal.cli.client.network.local.network.windows.asks = Windows fragt vielleicht, ob journal dein Netzwerk verwenden darf. bei einem Standardkonto muss jemand mit Administratorrechten es erlauben. wenn Windows nicht gefragt hat und ein Gerät dein Journal immer noch nicht erreichen kann, blockiert eine frühere Auswahl in Windows vielleicht journal. jemand mit Administratorrechten kann es in { $security } unter { $firewall } erlauben.

journal.cli.client.network.mark.none = die Identität deines Journals entsteht, wenn seine mark bei der Ersteinrichtung festgelegt wird.

journal.cli.client.network.mark.unavailable = die mark deines Journals: gerade nicht verfügbar

journal.cli.client.network.observer.pause = { $verb } ist noch nicht verfügbar.

journal.cli.client.network.operation = Vorgang: { $phase }

journal.cli.client.network.option.float = Fehler: die Option braucht einen Gleitkommawert.

journal.cli.client.network.pair.complete = Kopplungsanfrage abgeschlossen; das Gerät sollte in `{ $command }` erscheinen.

journal.cli.client.network.pair.device = Gerät: { $device }

journal.cli.client.network.pair.expired = Zeitüberschreitung. Kopplungscode abgelaufen.

journal.cli.client.network.pair.fingerprint = { $ca } Fingerabdruck: { $algorithm }{ $fingerprint }

journal.cli.client.network.pair.link = { $token }: { $link }

journal.cli.client.network.pair.paired = Gekoppelt: { $label }

journal.cli.client.network.pair.result.date = { $indent }Gekoppelt am:{ $indent } { $date }

journal.cli.client.network.pair.result.fingerprint = { $indent }Fingerabdruck: { $fingerprint }

journal.cli.client.network.pair.wait = Wartet auf ein gekoppeltes System…

journal.cli.client.network.portal.approve = fortfahren zum Genehmigen → { $url }

journal.cli.client.network.posture.direct = Lage: direkt

journal.cli.client.network.posture.private = Lage: privates Netzwerk

journal.cli.client.network.private.enrolled = registriert: ja

journal.cli.client.network.private.link.needs.subscription = dein privates Netzwerk ist noch nicht an. deine Einwilligung ist gespeichert. schalte es im Diensteportal fertig ein und schalte es dann hier erneut ein:

journal.cli.client.network.private.link.setting.up = dein privates Netzwerk wird eingerichtet...

journal.cli.client.network.private.link.subscribing = dein privates Netzwerk ist noch nicht an. deine Einwilligung ist gespeichert. schalte es im Diensteportal innerhalb der nächsten Stunde fertig ein, und dein Journal übernimmt es von selbst:

journal.cli.client.network.private.state.enabled = Zustand: aktiviert

journal.cli.client.network.private.state.inconsistent = Zustand: Reparatur nötig

journal.cli.client.network.private.state.not.enabled = Zustand: nicht aktiviert

journal.cli.client.network.private.state.other = Zustand: { $state }

journal.cli.client.network.private.state.unknown = Zustand: unbekannt

journal.cli.client.network.private.unenrolled = registriert: nein

journal.cli.client.network.reach.direct = Reach-Lage: direkt

journal.cli.client.network.reach.private = Reach-Lage: privates Netzwerk

journal.cli.client.network.relay.url = Relay-URL: { $url }

journal.cli.client.network.role.invalid = ungültige Rolle; erwartet wird eine von: { $phone }, { $observer }

journal.cli.client.network.status.enrolled = Registriert:      ja

journal.cli.client.network.status.network.enabled = Privates Netzwerk: aktiviert

journal.cli.client.network.status.network.inconsistent = Privates Netzwerk: Reparatur nötig

journal.cli.client.network.status.network.not.enabled = Privates Netzwerk: nicht aktiviert

journal.cli.client.network.status.network.other = Privates Netzwerk: { $state }

journal.cli.client.network.status.network.unknown = Privates Netzwerk: unbekannt

journal.cli.client.network.status.relay = Relay-URL:     { $url }

journal.cli.client.network.status.relay.absent = Relay-URL:     None

journal.cli.client.network.status.unenrolled = Registriert:      nein

journal.cli.client.network.time.day = { $quantity ->
    [one] vor { $quantity } Tag
   *[other] vor { $quantity } Tagen
    }

journal.cli.client.network.time.hour = { $quantity ->
    [one] vor { $quantity } Stunde
   *[other] vor { $quantity } Stunden
    }

journal.cli.client.network.time.minute = { $quantity ->
    [one] vor { $quantity } Minute
   *[other] vor { $quantity } Minuten
    }

journal.cli.client.network.time.month = { $quantity ->
    [one] vor { $quantity } Monat
   *[other] vor { $quantity } Monaten
    }

journal.cli.client.network.time.second = { $quantity ->
    [one] vor { $quantity } Sekunde
   *[other] vor { $quantity } Sekunden
    }

journal.cli.client.network.time.week = { $quantity ->
    [one] vor { $quantity } Woche
   *[other] vor { $quantity } Wochen
    }

journal.cli.client.network.timeout.integer = Fehler: die Option { $flag } braucht eine ganze Zahl.

journal.cli.client.network.unpaired = Entkoppelt.

journal.cli.client.option.required = Fehler: die Option { $option } ist erforderlich.

journal.cli.client.option.unknown = Fehler: unbekannte Option { $option }.

journal.cli.client.option.value = Fehler: die Option { $option } braucht ein Argument.

journal.cli.client.port.invalid = { $variable } muss eine ganze Zahl von 1 bis 65535 sein.

journal.cli.client.stdin.read = natives Lesen von { $brand } { $input } fehlgeschlagen: { $error }

journal.cli.client.transport.timeout = { $method } { $path } hat die lokale Zeitüberschreitung von { $service } überschritten (Verbindung={ $connect }s, Lesen={ $read }s, gesamt={ $total }s)

journal.cli.client.transport.timeout.open = { $method } { $path } hat die lokale Zeitüberschreitung von { $service } überschritten (Verbindung={ $connect }s, Lesen=keine, gesamt=keine)

journal.cli.client.trash.moved = '{ $name }' wurde in den Papierkorb verschoben.

journal.cli.client.trash.moved.day = '{ $name }' wurde in den Papierkorb verschoben. Es wird { $day } endgültig gelöscht, außer es wird wiederhergestellt.

journal.cli.client.trash.moved.failed = '{ $name }' wurde in den Papierkorb verschoben. Es konnte jetzt nicht endgültig gelöscht werden.

journal.cli.client.trash.moved.failed.day = '{ $name }' wurde in den Papierkorb verschoben. Es konnte jetzt nicht endgültig gelöscht werden. Es wird { $day } endgültig gelöscht, außer es wird wiederhergestellt.

journal.cli.importer.arg.archive.conflict.commands = unbekannte Argumente: widersprüchliche Audio-Archiv-Befehle

journal.cli.importer.arg.archive.conflict.flags = unbekannte Argumente: widersprüchliche Audio-Archiv-Flags

journal.cli.importer.arg.archive.identity.scope = --archive-identity gilt nur mit --audio-archive

journal.cli.importer.arg.archive.identity.sha256 = --archive-identity muss eine SHA-256-Identität sein

journal.cli.importer.arg.error = solstone journal importer: Fehler: { $detail }

journal.cli.importer.arg.expected.one = Argument { $argument }: ein Argument erwartet

journal.cli.importer.arg.facet.scope = --facet gilt nur beim Importieren einer einzelnen Audio- oder Textdatei oder eines Körper-Imports

journal.cli.importer.arg.invalid.int = Argument { $flag }: ungültiger int-Wert: '{ $value }'

journal.cli.importer.arg.media.required = die folgenden Argumente sind erforderlich: media

journal.cli.importer.arg.offset.limit.scope = unbekannte Argumente: --offset/--limit gelten nur mit --audio-archive-members

journal.cli.importer.arg.resume.scope = unbekannte Argumente: --resume gilt nur mit --audio-archive

journal.cli.importer.arg.source.zone.scope = unbekannte Argumente: --source-zone gilt nur mit --audio-archive

journal.cli.importer.arg.unrecognized = unbekannte Argumente: { $arguments }

journal.cli.importer.arg.usage = Aufruf: solstone journal importer [-h] [Optionen] media [timestamp]

journal.cli.importer.audio.archive.admitted.line1 = Archiv { $run_id }: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.changed = das Archiv hat sich nach der Vorschau geändert; zeig die Vorschau erneut an

journal.cli.importer.audio.archive.complete.line1 = Audio-Archiv { $operation }: { $run_id }

journal.cli.importer.audio.archive.complete.line2 = gespeichert { $parts } in { $tiles }; { $pending } ausstehend in der Verarbeitung, { $processed } verarbeitet, { $unavailable } nicht verfügbar, { $rejected } abgelehnt

journal.cli.importer.audio.archive.dry.run.resume = nutze --audio-archive-status, um einen angenommenen Lauf anzusehen

journal.cli.importer.audio.archive.failed = Audio-Archiv ist fehlgeschlagen: { $error }

journal.cli.importer.audio.archive.folder.needs.zone = ein Ordner erfordert --source-zone

journal.cli.importer.audio.archive.left.out.header = ausgelassen:

journal.cli.importer.audio.archive.left.out.inline = ausgelassen: { $counts }

journal.cli.importer.audio.archive.members.failed = Audio-Archiv-Einträge sind fehlgeschlagen: { $error }

journal.cli.importer.audio.archive.members.summary = Audio-Archiv-Einträge: run_id={ $run_id } offset={ $offset } count={ $count }

journal.cli.importer.audio.archive.preview.line1 = Audio-Archiv-Vorschau: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.progress.saved = gespeichert { $saved } von { $total } ({ $tiles })

journal.cli.importer.audio.archive.span.line = Zeitspanne: { $span }

journal.cli.importer.audio.archive.span.none = nichts

journal.cli.importer.audio.archive.span.range = { $first } bis { $last }

journal.cli.importer.audio.archive.status.failed = Audio-Archiv-Status ist fehlgeschlagen: { $error }

journal.cli.importer.audio.archive.status.summary = Audio-Archiv-Status: run_id={ $run_id } total_parts={ $total_parts } saved_parts={ $saved_parts } saved_tiles={ $saved_tiles } pending_processing={ $pending } processed_parts={ $processed } unavailable_parts={ $unavailable } rejected_parts={ $rejected } save_complete={ $save_complete }

journal.cli.importer.audio.archive.zone.changed = die Zeitzone hat sich nach der Vorschau geändert; zeig die Vorschau erneut an

journal.cli.importer.audio.complete = Allgemeiner Audio-Import abgeschlossen: { $outcome }

journal.cli.importer.audio.dropped.notice = ein Teil dieser Aufnahme konnte nicht importiert werden: { $ranges } in die Aufnahme. die anderen Teile sind in deinem Journal, und ein erneuter Import der Datei würde sie ein zweites Mal hinzufügen.

journal.cli.importer.audio.dropped.range = { $start } bis { $end }

journal.cli.importer.audio.failed = Audio-Import ist fehlgeschlagen: { $error }

journal.cli.importer.audio.manifest.failed = der Audio-Import wurde in deinem Journal gespeichert, aber diese Datei konnte nicht als importiert gekennzeichnet werden, daher würde ein erneuter Import sie ein zweites Mal hinzufügen: { $error }

journal.cli.importer.audio.processing.failed = Verarbeitung des Audio-Imports ist fehlgeschlagen: { $segments }

journal.cli.importer.audio.processing.stalled = der Audio-Import wurde noch verarbeitet, als der Importer aufgehört hat zu warten: { $segments }

journal.cli.importer.audio.runtime.failed = Laufzeitumgebung des Audio-Imports ist fehlgeschlagen: { $error }

journal.cli.importer.backends.header = Abgleichbare Backends:

journal.cli.importer.body.apple.already.ran = dieser Import ist bereits gelaufen, daher wurde nichts hereingeholt. starte einen neuen über die Karte von Apple Health.

journal.cli.importer.body.apple.card.only = --import-id gilt nur zum Speichern eines Exports von Apple Health über seine Karte, daher wurde nichts hereingeholt.

journal.cli.importer.body.apple.preview.intro = das sieht nach einem Export von Apple Health aus. das Speichern liest { $head }, und { $tail }.

journal.cli.importer.body.apple.preview.moves = { $quantity ->
    [one] ja zu sagen kann bis zu 1 Datensatz verschieben, der bereits unter einer anderen Facette aufgeführt ist.
   *[other] ja zu sagen kann bis zu { $count } Datensätze verschieben, die bereits unter einer anderen Facette aufgeführt sind.
}

journal.cli.importer.body.apple.preview.span = es läuft von { $first } bis { $last }: { $workouts }, { $nights } aus Schlafphasen zusammengesetzt, { $mindful }, { $readings }, { $daily } und { $logged }.

journal.cli.importer.body.apple.reads.daily = Tagessummen wie Schritte und Strecke

journal.cli.importer.body.apple.reads.logged = protokollierte Einträge wie Essen und Symptome

journal.cli.importer.body.apple.reads.mindful = Achtsamkeitszeit

journal.cli.importer.body.apple.reads.readings = Herzfrequenz und andere Messwerte

journal.cli.importer.body.apple.reads.sleep = Schlaf

journal.cli.importer.body.apple.reads.workouts = Trainings

journal.cli.importer.body.apple.unknown.import = --import-id nennt keinen Import, daher wurde nichts hereingeholt.

journal.cli.importer.body.apple.whole = ein Import von Apple Health liest immer den ganzen Export, daher akzeptiert er --date-from, --date-to oder --force nicht. nichts wurde gespeichert; führe ihn erneut ohne sie aus.

journal.cli.importer.body.apple.wrong.export = dieser Import hält diesen Export nicht, daher wurde nichts hereingeholt.

journal.cli.importer.body.days.hidden = { $quantity ->
    [one] 1 Tag mit Körperdaten wurde im Archiv ausgeblendet, daher ist er auch in diesem Journal ausgeblendet.
   *[other] { $count } Tage mit Körperdaten wurden im Archiv ausgeblendet, daher sind sie auch in diesem Journal ausgeblendet.
}

journal.cli.importer.body.facet.ask.existing = deine Trainings, deinen Schlaf und andere Körperdaten unter der Facette einordnen, die du { $facet } genannt hast?

journal.cli.importer.body.facet.ask.make.body = eine Körper-Facette für deine Trainings, deinen Schlaf und andere Körperdaten anlegen?

journal.cli.importer.body.facet.ask.unmute = deine Körper-Facette ist stummgeschaltet. schalte sie für deine Trainings, deinen Schlaf und andere Körperdaten wieder ein?

journal.cli.importer.body.facet.moves.body = { $quantity ->
    [one] ein Ja kann bis zu 1 Datensatz, der bereits auf der Aktivitätenseite aufgeführt ist, von { $from } verschieben.
   *[other] ein Ja kann bis zu { $count } Datensätze, die bereits auf der Aktivitätenseite aufgeführt sind, von { $from } verschieben.
}

journal.cli.importer.body.facet.moves.strava = { $quantity ->
    [one] ein Ja verschiebt 1 bereits eingeordnetes Training, von { $from }.
   *[other] ein Ja verschiebt { $count } bereits eingeordnete Trainings, von { $from }.
}

journal.cli.importer.body.facet.no.means.body = wenn du nein sagst, werden neue Trainings und neuer Schlaf nicht auf der Aktivitätenseite aufgeführt, und Agenten, die auf manche Facetten beschränkt sind, sehen sie nicht. die Suche und die Körper-App zeigen sie weiterhin.

journal.cli.importer.body.facet.no.means.strava = wenn du nein sagst, werden neue Trainings nicht auf der Aktivitätenseite aufgeführt, keine Personen oder Dinge kommen aus ihnen, und Agenten, die auf manche Facetten beschränkt sind, sehen sie nicht. die Suche und die Körper-App zeigen sie weiterhin.

journal.cli.importer.body.facet.prompt.yn = { $question } [y/n]

journal.cli.importer.body.facet.save.failed = deine Antwort konnte nicht gespeichert werden: { $error }

journal.cli.importer.body.oura.bad.date = { $flag } erwartet ein Datum wie 2026-01-31, und { $text } ist keines. nichts wurde gelesen.

journal.cli.importer.body.oura.date.to.needs.from = --date-to braucht ein --date-from dazu. nichts wurde gelesen.

journal.cli.importer.body.oura.range.future = dieser Bereich liegt nach heute, und Oura hat dort noch nichts. nichts wurde gelesen.

journal.cli.importer.body.oura.window.conflict = nutze --window-days oder Datumsangaben, nicht beides. nichts wurde gelesen.

journal.cli.importer.body.oura.window.zero = --window-days braucht mindestens einen Tag. nichts wurde gelesen.

journal.cli.importer.body.zones.hours = { $quantity ->
    [one] 1 Stunde von { $source } kann zur falschen Zeit erscheinen: sie wurde in einer anderen Zeitzone gespeichert als diejenige, die dieses Journal für { $source } hält.
   *[other] { $count } Stunden von { $source } können zur falschen Zeit erscheinen: sie wurden in einer anderen Zeitzone gespeichert als diejenige, die dieses Journal für { $source } hält.
}

journal.cli.importer.body.zones.under.hour = weniger als eine Stunde von { $source } kann zur falschen Zeit erscheinen: sie wurde in einer anderen Zeitzone gespeichert als diejenige, die dieses Journal für { $source } hält.

journal.cli.importer.connect.available = Verbindbare Backends: oura, plaud

journal.cli.importer.connect.oura.both.required = Oura braucht sowohl die Client-ID als auch das Client-Geheimnis, daher wurde nichts gespeichert.

journal.cli.importer.connect.oura.done = Oura ist verbunden, und die Verbindung ist in den Einstellungen dieses Journals gespeichert.

journal.cli.importer.connect.oura.scopes = angefragte Berechtigungen: { $scopes }

journal.cli.importer.connect.unknown.backend = Unbekanntes Verbindungs-Backend: { $backend }

journal.cli.importer.count.attendee = { $quantity ->
    [one] 1 eindeutige teilnehmende Person
   *[other] { $count } eindeutige teilnehmende Personen
}

journal.cli.importer.count.calendar.event = { $quantity ->
    [one] 1 Kalendertermin
   *[other] { $count } Kalendertermine
}

journal.cli.importer.count.conversation = { $quantity ->
    [one] 1 Unterhaltung
   *[other] { $count } Unterhaltungen
}

journal.cli.importer.count.daily.total = { $quantity ->
    [one] 1 Tagessumme
   *[other] { $count } Tagessummen
}

journal.cli.importer.count.day = { $quantity ->
    [one] 1 Tag
   *[other] { $count } Tage
}

journal.cli.importer.count.entity = { $quantity ->
    [one] 1 Entität
   *[other] { $count } Entitäten
}

journal.cli.importer.count.event = { $quantity ->
    [one] 1 Ereignis
   *[other] { $count } Ereignisse
}

journal.cli.importer.count.facet = { $quantity ->
    [one] 1 Facette
   *[other] { $count } Facetten
}

journal.cli.importer.count.logged.entry = { $quantity ->
    [one] 1 protokollierter Eintrag
   *[other] { $count } protokollierte Einträge
}

journal.cli.importer.count.member = { $quantity ->
    [one] 1 Eintrag
   *[other] { $count } Einträge
}

journal.cli.importer.count.message = { $quantity ->
    [one] 1 Nachricht
   *[other] { $count } Nachrichten
}

journal.cli.importer.count.mindful.session = { $quantity ->
    [one] 1 Achtsamkeitssitzung
   *[other] { $count } Achtsamkeitssitzungen
}

journal.cli.importer.count.night = { $quantity ->
    [one] 1 Nacht
   *[other] { $count } Nächte
}

journal.cli.importer.count.part = { $quantity ->
    [one] 1 Teil
   *[other] { $count } Teile
}

journal.cli.importer.count.pdf.document = { $quantity ->
    [one] 1 PDF-Dokument
   *[other] { $count } PDF-Dokumente
}

journal.cli.importer.count.reading = { $quantity ->
    [one] 1 Messwert
   *[other] { $count } Messwerte
}

journal.cli.importer.count.recording = { $quantity ->
    [one] 1 Aufnahme
   *[other] { $count } Aufnahmen
}

journal.cli.importer.count.segment = { $quantity ->
    [one] 1 Segment
   *[other] { $count } Segmente
}

journal.cli.importer.count.source.byte = { $quantity ->
    [one] 1 Quell-Byte
   *[other] { $count } Quell-Bytes
}

journal.cli.importer.count.tile = { $quantity ->
    [one] 1 Kachel
   *[other] { $count } Kacheln
}

journal.cli.importer.count.total.page = { $quantity ->
    [one] 1 Seite insgesamt
   *[other] { $count } Seiten insgesamt
}

journal.cli.importer.count.warning = { $quantity ->
    [one] 1 Warnung
   *[other] { $count } Warnungen
}

journal.cli.importer.count.workout = { $quantity ->
    [one] 1 Training
   *[other] { $count } Trainings
}

journal.cli.importer.desktop.exe.no.parent = die aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.importer.desktop.missing.sibling = fehlende nebengeordnete Programmdatei { $path }

journal.cli.importer.embedded.body.refusal = Exporte von Apple Health und Oura-Daten werden von einem Journal auf einem Computer hereingeholt, daher wurde nichts hereingeholt.

journal.cli.importer.error.apple.inspect = der Export von Apple Health konnte nicht geprüft werden; versuch es erneut mit einem gültigen Export

journal.cli.importer.error.facet.not.unmuted = "{ $facet }" ist keine deiner nicht stummgeschalteten Facetten, daher wurde nichts importiert. deine nicht stummgeschalteten Facetten: { $enabled }

journal.cli.importer.error.facet.scope = --facet gilt nur für eine Audiodatei, eine reine Textdatei oder einen Körper-Import, daher wurde nichts importiert; importiere diesen ohne sie

journal.cli.importer.error.facets.unreadable = deine Facetten konnten nicht gelesen werden, daher wurde nichts importiert: { $error }

journal.cli.importer.error.no.facet.conflict = Argument --no-facet: nicht zusammen mit dem Argument --facet erlaubt

journal.cli.importer.error.no.facet.scope = --no-facet gilt nur für einen Körper-Import, daher wurde nichts importiert; importiere diesen ohne sie

journal.cli.importer.error.preview.failed = der Import konnte nicht als Vorschau angezeigt werden: { $error }

journal.cli.importer.error.stopped = Import gestoppt

journal.cli.importer.error.timestamp.format = der Zeitstempel muss das Format YYYYMMDD_HHMMSS haben

journal.cli.importer.error.timestamp.format.offset = der Zeitstempel muss YYYYMMDD_HHMMSS sein oder ein Datum und eine Uhrzeit mit Offset, zum Beispiel 2025-11-02T01:30:00-07:00

journal.cli.importer.error.unknown.source = unbekannte Importer-Quelle: { $source }

journal.cli.importer.error.zone.changed.preview = die Zeitzone hat sich nach der Vorschau geändert; zeig die Vorschau erneut an, bevor du importierst

journal.cli.importer.force.day.notice = Hinweis: diese Datei datiert auf { $fresh }, und ein früherer Import hat { $stored } genutzt. dieser --force-Import nutzt { $planned }. bestehende Aufnahmen werden nicht verschoben.

journal.cli.importer.force.pdf.day.notice = Hinweis: der Name dieser Datei datiert auf { $fresh }, und ein früherer Import wurde am { $stored } verzeichnet. die eigenen Datumsangaben des PDF können einen anderen Tag bestimmen. wähle --timestamp, um das Datum des PDF zu setzen. bestehende Aufnahmen werden nicht verschoben.

journal.cli.importer.generic.text.complete = Allgemeiner Text-Import abgeschlossen: segments={ $segments }

journal.cli.importer.generic.text.untimed.line1 = keine Zeiten für Redebeiträge in dieser Datei gefunden, daher ist sie als ein Segment zur Startzeit des Imports gelandet, jede Zeile wie geschrieben.

journal.cli.importer.generic.text.untimed.line2 = wenn deine Transkripte Zeiten auf eine Art markieren, die das Journal noch nicht liest, frag uns, das zu ergänzen: { $url }

journal.cli.importer.help.opt.archive.members = { $indent }Einträge eines Audio-Archiv-Laufs auflisten (mit --offset und --limit)

journal.cli.importer.help.opt.archive.status = { $indent }Kompakten Status eines Audio-Archiv-Laufs ausgeben

journal.cli.importer.help.opt.audio.archive.line1 = { $indent }--audio-archive PATH  Einen Ordner mit Aufnahmen in Ogg Opus oder ein JSON-Manifest importieren

journal.cli.importer.help.opt.audio.archive.line2 = { $indent }Ordner: Limitless-Dateinamen und --source-zone sind erforderlich.

journal.cli.importer.help.opt.audio.archive.line3 = { $indent }Das Manifest: { $example }

journal.cli.importer.help.opt.audio.archive.line4 = { $indent }Oder nutze local_civil und source_zone für die Zeit eines Eintrags.

journal.cli.importer.help.opt.audio.archive.line5 = { $indent }Pfade sind absolut oder relativ zum Manifest.

journal.cli.importer.help.opt.audio.archive.line6 = { $indent }Ausdrückliche Manifest-Zeiten haben Vorrang vor Dateinamen.

journal.cli.importer.help.opt.audio.archive.line7 = { $indent }Unterstützt Ogg Opus mit einem Stream, Mono oder Stereo; andere Container werden ausgelassen.

journal.cli.importer.help.opt.audio.archive.line8 = { $indent }Zeigt die Größe vor dem Speichern; --dry-run speichert nichts.

journal.cli.importer.help.opt.auto = { $indent }--auto [AUTO]         Den erkannten Zeitstempel übernehmen; AUTO ist ein Hinweis, nicht der Dateipfad

journal.cli.importer.help.opt.backends = { $indent }--backends            Abgleichbare Importer-Backends auflisten

journal.cli.importer.help.opt.confirm.body = { $indent }--confirm-body-save   Bestätigen, dass dieser Lauf sensible Ausgabe des Körper-Importers speichern darf

journal.cli.importer.help.opt.debug = { $indent }-d, --debug            Debug-Protokollierung aktivieren

journal.cli.importer.help.opt.deterministic.line1 = { $indent }--deterministic-only  Wenn kein Datum aus der Datei gelesen werden kann, sie überspringen (exit 0)

journal.cli.importer.help.opt.deterministic.line2 = { $indent }statt fehlzuschlagen

journal.cli.importer.help.opt.dry.run = { $indent }--dry-run             Anzeigen, was importiert würde, ohne ins Journal zu schreiben

journal.cli.importer.help.opt.facet.line1 = { $indent }--facet FACET         Diese Audio- oder Textdatei oder diesen Körper-Import unter FACET einordnen,

journal.cli.importer.help.opt.facet.line2 = { $indent }eine deiner nicht stummgeschalteten Facetten, statt dein Journal wählen zu lassen

journal.cli.importer.help.opt.facet.line3 = { $indent }(bei einem Körper-Import: deine gemerkte Facette, oder keine)

journal.cli.importer.help.opt.force = { $indent }--force               Neuimport erzwingen; Körper-Quellen erstellen einen separaten Import

journal.cli.importer.help.opt.help = { $indent }-h, --help            diese Hilfemeldung anzeigen und beenden

journal.cli.importer.help.opt.json.line1 = { $indent }--json                Ergebnisse als JSON ausgeben; Speicherungen eines Archivs geben Fortschrittsobjekte

journal.cli.importer.help.opt.json.line2 = { $indent }und ein Endergebnis aus, ein JSON-Objekt pro Zeile.

journal.cli.importer.help.opt.limit = { $indent }--limit LIMIT         Höchstzahl aufzulistender Einträge (Vorgabe 100, höchstens 1000)

journal.cli.importer.help.opt.list.importers = { $indent }--list-importers      Verfügbare Datei-Importer auflisten

journal.cli.importer.help.opt.no.facet.line1 = { $indent }--no-facet            Diesen Körper-Import unter keiner Facette einordnen, wie auch immer die gemerkte

journal.cli.importer.help.opt.no.facet.line2 = { $indent }Vorgabe ist

journal.cli.importer.help.opt.offset = { $indent }--offset OFFSET       Anfangs-Offset für das Auflisten der Einträge

journal.cli.importer.help.opt.resume = { $indent }--resume IMPORT_ID    Nicht gespeicherte Teile des eingefrorenen Plans eines Audio-Archivs füllen

journal.cli.importer.help.opt.save = { $indent }--save                Mit --sync: neue Dateien herunterladen und importieren (Vorgabe ist der Probelauf)

journal.cli.importer.help.opt.source = { $indent }--source SOURCE       Erforderlich für klassifizierte Formate (ics, Chat-Export, Bild, PDF)

journal.cli.importer.help.opt.source.zone = { $indent }--source-zone IANA    Zeitzone für Audio-Archive aus Ordnern oder bürgerliche Zeiten (z. B. America/New_York)

journal.cli.importer.help.opt.timestamp = { $indent }Allgemeine .txt/.md nutzen dies, oder --auto, um einen erkannten Zeitstempel zu übernehmen (--timestamp YYYYMMDD_HHMMSS oder ein Datum und eine Uhrzeit mit Offset, zum Beispiel 2025-11-02T01:30:00-07:00)

journal.cli.importer.help.opt.verbose = { $indent }-v, --verbose         Ausführliche Ausgabe aktivieren

journal.cli.importer.help.options.header = Optionen:

journal.cli.importer.help.positional.header = Positionsargumente:

journal.cli.importer.help.positional.media = { $indent }media                 Pfad zu einer Audio- oder Textdatei

journal.cli.importer.help.summary = Eine Mediendatei ins Journal importieren

journal.cli.importer.help.usage = Aufruf: solstone journal importer [-h] [--timestamp TIMESTAMP] [--facet FACET] [--no-facet]

journal.cli.importer.list.desc.apple.health = Daten aus export.xml von Apple Health als Vorschau anzeigen, ohne ins Journal zu schreiben

journal.cli.importer.list.desc.chatgpt = Unterhaltungen aus einem ChatGPT-Export importieren

journal.cli.importer.list.desc.claude = Unterhaltungen aus einem Claude-Chat-Export importieren

journal.cli.importer.list.desc.document = PDF-Dokumente mit Text- und Rasterextraktion über einen Worker importieren

journal.cli.importer.list.desc.evernote = Notizen aus einem Evernote-Export

journal.cli.importer.list.desc.gemini = Unterhaltungen aus einem Gemini- oder Bard-Export von Google Takeout importieren

journal.cli.importer.list.desc.ics = Ereignisse aus ICS-Kalenderdateien oder einem ZIP-Export von Google Calendar importieren

journal.cli.importer.list.desc.image = Ein einzelnes Bild importieren und seinen Inhalt mit Vision beschreiben

journal.cli.importer.list.desc.journal.archive = Ein exportiertes Journal-Archiv in das aktuelle Journal zusammenführen

journal.cli.importer.list.desc.oura = JSON-Dokumente der Oura API v2 als Vorschau anzeigen (synthetische Fixtures; der Speicherpfad ist eine spätere, freigabepflichtige Phase)

journal.cli.importer.list.desc.strava = Trainings aus einem Strava-Download (gelesen wird nur activities.csv)

journal.cli.importer.list.header = Datei-Importer:

journal.cli.importer.notes.folder.retired = das Importieren eines Ordners mit Notizen gibt es nicht mehr; ein Obsidian-Tresor kommt in dein Journal, wenn du ihn in der solstone-App auf dem Computer einschaltest, auf dem der Tresor liegt

journal.cli.importer.oura.app.needed = dieses Journal ist noch nicht mit einer App verbunden, die du bei Oura registriert hast. registriere eine, führe dann `solstone journal importer --connect oura` in einem Terminal aus und füge ihre Client-ID und ihr Client-Geheimnis ein. so: { $how }

journal.cli.importer.oura.app.prompt.client.id = Client-ID:

journal.cli.importer.oura.app.prompt.client.secret = Client-Geheimnis:

journal.cli.importer.oura.app.prompt.intro = dieses Journal ist noch nicht mit einer App verbunden, die du bei Oura registriert hast. so verbindest du eine:

journal.cli.importer.oura.app.prompt.note = sie werden in den Einstellungen dieses Journals behalten und nur genutzt, um die Verbindung zu Oura herzustellen. nichts wird gespeichert, bis Oura ja sagt. so: { $how }

journal.cli.importer.oura.app.prompt.step1 = { $indent }1. melde dich unter https://cloud.ouraring.com/oauth/applications an und erstelle eine App

journal.cli.importer.oura.app.prompt.step2 = { $indent }2. füge die Redirect-URI http://localhost:8765/callback hinzu

journal.cli.importer.oura.app.prompt.step3 = { $indent }3. füge unten die Client-ID und das Client-Geheimnis der App ein

journal.cli.importer.oura.connect.failed.browser = ein Browser konnte hier nicht geöffnet werden. führe das auf dem Computer des Journals aus, nicht über eine entfernte Sitzung.

journal.cli.importer.oura.connect.failed.callback = die Anmeldung von Oura kommt auf Port 8765 zu diesem Computer zurück, und etwas anderes nutzt ihn. schließe das und führe es erneut aus.

journal.cli.importer.oura.connect.failed.declined = Oura hat die Verbindung nicht hergestellt, daher wurde nichts gespeichert.

journal.cli.importer.oura.connect.failed.exchange = Oura hat die Verbindung nicht angenommen, daher wurde nichts gespeichert. prüfe das Client-Geheimnis auf der Seite deiner App bei Oura und führe es erneut aus.

journal.cli.importer.oura.connect.failed.timeout = die Anmeldung bei Oura wurde nicht innerhalb von fünf Minuten abgeschlossen, daher wurde nichts gespeichert. führe es erneut aus, um es noch einmal zu versuchen.

journal.cli.importer.plaud.begin.invalid.redirect = ungültige Plaud-Callback-Redirect-URI

journal.cli.importer.plaud.begin.loopback.bind = der lokale Callback-Listener konnte nicht gebunden werden

journal.cli.importer.plaud.begin.random.failed = die Anmeldekennung konnte nicht erzeugt werden

journal.cli.importer.plaud.connect.detail.busy = eine andere Anmeldung oder ein anderer Abgleich läuft bereits

journal.cli.importer.plaud.connect.detail.callback.invalid = die Anmeldeadresse war ungültig

journal.cli.importer.plaud.connect.detail.cancelled = Anmeldung abgebrochen

journal.cli.importer.plaud.connect.detail.denied = die Anmeldung wurde nicht genehmigt

journal.cli.importer.plaud.connect.detail.exchange.failed = die Anmeldung konnte nicht abgeschlossen werden

journal.cli.importer.plaud.connect.detail.expired = der Anmeldeversuch hat die Zeit überschritten

journal.cli.importer.plaud.connect.detail.storage = die Anmeldung konnte in diesem Journal nicht gespeichert werden

journal.cli.importer.plaud.connect.done = { $provider } ist verbunden, und deine Anmeldung ist in diesem Journal gespeichert.

journal.cli.importer.plaud.connect.done.account = { $provider } ist verbunden als { $account }, und deine Anmeldung ist in diesem Journal gespeichert.

journal.cli.importer.plaud.connect.failed = { $provider } konnte nicht verbunden werden: { $detail }

journal.cli.importer.plaud.connect.next = führe { $command } aus, um jetzt Aufnahmen hereinzuholen, oder schalte den stündlichen Abgleich in den Einstellungen ein.

journal.cli.importer.plaud.connect.open = öffne diesen Link, um dich mit { $provider } anzumelden:

journal.cli.importer.plaud.connect.retry.url = das war nicht die Anmeldeadresse von { $provider }. füge die vollständige Adresse aus der Adressleiste deines Browsers ein oder schließe sie im Browser ab.

journal.cli.importer.plaud.connect.waiting = warte, bis { $provider } die Anmeldung abschließt. wenn dein Browser auf einem anderen Gerät ist, füge die endgültige Adresse aus der Adressleiste ein und drück die Eingabetaste:

journal.cli.importer.plaud.failure.audio.not.ready = der Audio-Download-Link ist noch nicht bereit; beim nächsten Abgleich wird es erneut versucht

journal.cli.importer.plaud.failure.catalogue = Katalog ist fehlgeschlagen

journal.cli.importer.plaud.failure.download = Download ist fehlgeschlagen

journal.cli.importer.plaud.failure.manifest = Import-Zuordnung ist fehlgeschlagen

journal.cli.importer.plaud.failure.pipeline = Import ist fehlgeschlagen

journal.cli.importer.plaud.failure.temporary.url = die Download-URL konnte nicht geholt werden

journal.cli.importer.plaud.failure.token.refused = die Anmeldung ist abgelaufen oder wurde widerrufen; melde dich in den Einstellungen erneut an

journal.cli.importer.plaud.preview.complete = Plaud-Abgleich-Vorschau abgeschlossen: files={ $files }

journal.cli.importer.plaud.sync.busy = dein Journal prüft { $provider } bereits. versuch es noch einmal, wenn das fertig ist.

journal.cli.importer.plaud.sync.missing.credential = Plaud-Zugangsdaten sind nicht konfiguriert

journal.cli.importer.plaud.sync.operation = Plaud: { $detail }

journal.cli.importer.plaud.sync.state.failed = Plaud-Abgleichsstatus ist fehlgeschlagen: { $message }

journal.cli.importer.preview.chatgpt = { $messages } aus einem ChatGPT-Export

journal.cli.importer.preview.claude = { $messages } aus einem Claude-Chat-Export

journal.cli.importer.preview.document.counts = { $documents }, { $pages }

journal.cli.importer.preview.document.none = Keine PDF-Dokumente gefunden

journal.cli.importer.preview.document.unreadable = { $documents }, { $pages }; { $unreadable } unlesbar ({ $failures })

journal.cli.importer.preview.gemini = { $messages } aus einem Gemini-Export

journal.cli.importer.preview.ics.counts = { $events }, { $attendees }

journal.cli.importer.preview.ics.no.events = Keine Ereignisse in ICS-Daten gefunden

journal.cli.importer.preview.ics.none = Keine ICS-Daten gefunden

journal.cli.importer.preview.image.one = 1 Bild ({ $format }, { $width }×{ $height })

journal.cli.importer.registry.archive.error.and.record.failed = { $source }-Import ist fehlgeschlagen: { $error }; der Versuch konnte nicht protokolliert werden: { $record_error }

journal.cli.importer.registry.archive.missing.facts = { $source }-Import ist fehlgeschlagen: fehlende Fakten zum Versuch für { $attempt }

journal.cli.importer.registry.archive.missing.provenance = { $source }-Import ist fehlgeschlagen: fehlende Herkunft für Versuch { $attempt }

journal.cli.importer.registry.archive.record.attempt.failed = { $source }-Import ist fehlgeschlagen: der Versuch konnte nicht protokolliert werden: { $error }

journal.cli.importer.registry.archive.record.completed.failed = { $source }-Import ist fehlgeschlagen: der abgeschlossene Versuch konnte nicht protokolliert werden: { $error }

journal.cli.importer.registry.archive.record.merge.failed = { $source }-Import ist fehlgeschlagen: die Ergebnisse der Zusammenführung konnten nicht protokolliert werden: { $error }

journal.cli.importer.registry.attempt.superseded = { $source }-Import ist fehlgeschlagen: Versuch { $import_id }:{ $generation } wurde abgelöst

journal.cli.importer.registry.evernote.attachments.failed = Evernote-Import ist fehlgeschlagen. { $error }

journal.cli.importer.registry.found.nothing = { $source }-Import ist fehlgeschlagen: nichts zum Importieren gefunden

journal.cli.importer.registry.gemini.folder.unreadable = Gemini-Import ist fehlgeschlagen. der gespeicherte Export-Ordner konnte nicht gelesen werden.

journal.cli.importer.registry.originals.choice.unreadable = die Auswahl, Originale zu behalten, konnte nicht gelesen werden. öffne den Import erneut, um es noch einmal zu versuchen.

journal.cli.importer.registry.originals.cleanup.failed = in deinem Journal gespeichert. die zusätzliche Kopie konnte nicht entfernt werden.

journal.cli.importer.registry.originals.keep.failed = eine Kopie des Originals konnte nicht behalten werden. versuch es noch einmal.

journal.cli.importer.registry.preview.failed = Import-Vorschau ist fehlgeschlagen: { $error }

journal.cli.importer.registry.strava.discard.failed = der bereitgestellte Strava-Download konnte nicht entfernt werden: { $error }

journal.cli.importer.registry.strava.filing.failed = { $source }-Import ist fehlgeschlagen: die Trainings konnten nicht eingeordnet werden: { $error }

journal.cli.importer.registry.strava.release.failed = Trainings, die zur falschen Zeit liegen, konnten nicht freigegeben werden

journal.cli.importer.registry.summary.image = importiert: 1 Bild

journal.cli.importer.registry.summary.pdf = importiert: { $documents }

journal.cli.importer.registry.unconfirmed = { $source }-Import wurde in deinem Journal gespeichert, aber er konnte nicht bestätigen, dass die Einträge für die Suche bereit sind; führe ihn erneut aus

journal.cli.importer.size.listed.moves = { $quantity ->
    [one] 1 Training, das bereits auf der Aktivitätenseite aufgeführt ist, würde von { $from } zur Facette dieses Imports wechseln
   *[other] { $count } Trainings, die bereits auf der Aktivitätenseite aufgeführt sind, würden von { $from } zur Facette dieses Imports wechseln
}

journal.cli.importer.size.recordings.across = { $recordings } über { $days }

journal.cli.importer.size.skipped = { $count } übersprungen

journal.cli.importer.skipped.already = Import übersprungen: diese Datei wurde bereits importiert; nutze --force, um sie erneut zu importieren

journal.cli.importer.skipped.confirm.date = Import übersprungen: das Datum der Datei muss bestätigt werden; führe ihn erneut aus mit --timestamp oder --auto

journal.cli.importer.skipped.no.date = Import übersprungen: aus der Datei konnte kein Datum gelesen werden; nenne eines mit --timestamp YYYYMMDD_HHMMSS

journal.cli.importer.source.archive.already.present = { $source }-Import hat nichts zusammengeführt: der Archivinhalt ist bereits vorhanden

journal.cli.importer.source.archive.complete = { $source }-Import abgeschlossen: segments_copied={ $segments_copied } imports_copied={ $imports_copied } entities_created={ $entities_created } entities_merged={ $entities_merged } facets_created={ $facets_created } facets_merged={ $facets_merged }

journal.cli.importer.source.archive.incomplete = { $source }-Import unvollständig: { $detail }

journal.cli.importer.source.complete = { $source }-Import abgeschlossen: entries_written={ $entries } files_created={ $files } errors={ $errors } summary={ $summary }

journal.cli.importer.source.failed = { $source }-Import ist fehlgeschlagen: { $detail }

journal.cli.importer.source.preview = { $source }-Vorschau: date_range={ $start }..{ $end } items={ $items } entities={ $entities } summary={ $summary }

journal.cli.importer.summary.archive = Export des Journals: { $days }, { $entities }, { $facets } ({ $warnings })

journal.cli.importer.summary.conversations = importiert: { $messages } aus { $conversations } über { $days }

journal.cli.importer.summary.document = importiert: { $documents } über { $days } in { $segments }

journal.cli.importer.summary.ics.base = importiert: { $events } über { $days }

journal.cli.importer.summary.ics.over.limit = ; { $count } nicht nach dem Tag aufgeführt, an dem sie stattfinden, über die { $limit } Datumsangaben hinaus, die ein Import aufführt

journal.cli.importer.summary.ics.reason.invalid = { $count } unlesbar

journal.cli.importer.summary.ics.reason.no.change = { $count } ohne Änderungszeit

journal.cli.importer.summary.ics.reason.unknown.zone = { $count } in einer unbekannten Zeitzone

journal.cli.importer.summary.ics.skipped = ; { $skipped } nicht nach dem Tag aufgeführt, an dem sie stattfinden ({ $reasons })

journal.cli.importer.summary.ics.truncated.series = { $quantity ->
    [one] ; 1 sich wiederholende Serie nur für ihre letzten { $limit } Datumsangaben aufgeführt
   *[other] ; { $count } sich wiederholende Serien nur für ihre letzten { $limit } Datumsangaben aufgeführt
}

journal.cli.importer.summary.ics.unexpandable.series = { $quantity ->
    [one] ; 1 sich wiederholende Serie nur für ihr erstes Datum aufgeführt
   *[other] ; { $count } sich wiederholende Serien nur für ihr erstes Datum aufgeführt
}

journal.cli.importer.summary.image.none = Kein lesbares Bild gefunden

journal.cli.importer.sync.available = Verfügbare Backends: { $backends }

journal.cli.importer.sync.complete = { $backend }-Abgleich abgeschlossen: saved={ $saved } errors={ $errors }

journal.cli.importer.sync.unknown.backend = Unbekanntes Abgleich-Backend: { $backend }

journal.cli.importer.text.failed = Text-Import ist fehlgeschlagen: { $error }

journal.cli.importer.text.unconfirmed = der Text-Import wurde in deinem Journal gespeichert, aber er konnte nicht bestätigen, dass die Einträge für die Suche bereit sind; führe ihn erneut aus

journal.cli.importer.timestamp.confirm = erkannter Zeitstempel { $timestamp }; führe erneut aus mit --timestamp { $timestamp } oder --auto

journal.cli.oauth.callback.failure = die Anmeldung wurde nicht abgeschlossen. geh zurück zu deinem Journal und versuch es noch einmal

journal.cli.oauth.callback.success = angemeldet. du kannst diesen Tab schließen und zu deinem Journal zurückgehen

journal.cli.oauth.oura.callback.allowed = du hast es bei Oura erlaubt. du kannst diesen Tab schließen; das Terminal sagt, wenn es verbunden ist.

journal.cli.oauth.oura.callback.declined = Oura wurde nicht verbunden. du kannst diesen Tab schließen.

journal.cli.oauth.oura.callback.rejected = Oura-Autorisierung abgelehnt.

journal.cli.setup.clean.changed.during.service = die Installation hat sich geändert, während ihr Dienst entfernt wurde

journal.cli.setup.clean.child.exited = Deinstallation des Dienstes beendet mit { $code }

journal.cli.setup.clean.child.exited.detail = Deinstallation des Dienstes beendet mit { $code }: { $detail }

journal.cli.setup.clean.child.timed.out = Deinstallation des Dienstes beendet mit { $code } (Zeitüberschreitung)

journal.cli.setup.clean.child.timed.out.detail = Deinstallation des Dienstes beendet mit { $code } (Zeitüberschreitung): { $detail }

journal.cli.setup.clean.config.invalid.toml = Konfiguration ist kein gültiges TOML

journal.cli.setup.clean.config.io = Konfiguration konnte nicht gelesen werden: { $error }

journal.cli.setup.clean.config.missing.journal = Konfiguration hat keinen Schlüssel journal

journal.cli.setup.clean.config.not.absolute = journal-Pfad der Konfiguration ist nicht absolut

journal.cli.setup.clean.config.not.string = journal-Wert der Konfiguration ist keine Zeichenkette

journal.cli.setup.clean.confirm.config = { $indent }[{ $marker }] Konfiguration: { $path }

journal.cli.setup.clean.confirm.header = solstone journal setup --clean-uninstall wird diese Laufzeit-Artefakte entfernen:

journal.cli.setup.clean.confirm.keep.header = wird nicht entfernt:

journal.cli.setup.clean.confirm.keep.journal = { $indent }- Journal-Verzeichnis: { $path }

journal.cli.setup.clean.confirm.keep.macos.app = { $indent }- Pfad: /Applications/solstone.app

journal.cli.setup.clean.confirm.keep.macos.permissions = { $indent }- macOS-Berechtigungen für Mikrofon oder Bildschirmaufnahme

journal.cli.setup.clean.confirm.keep.macos.support = { $indent }- Pfad: ~/Library/Application Support/solstone/

journal.cli.setup.clean.confirm.keep.python = { $indent }- eine übrig gebliebene pip-, uv- oder pipx-Installation von journal

journal.cli.setup.clean.confirm.manifest = { $indent }[{ $marker }] Journal-Manifest: { $path }

journal.cli.setup.clean.confirm.marker.absent = nicht vorhanden

journal.cli.setup.clean.confirm.marker.present = vorhanden

journal.cli.setup.clean.confirm.marker.retain = behalten

journal.cli.setup.clean.confirm.service = { $indent }[{ $marker }] Dienst: { $path }

journal.cli.setup.clean.confirm.service.unavailable = { $indent }Dienstort nicht verfügbar: { $error }

journal.cli.setup.clean.confirm.wrapper = { $indent }[{ $marker }] Wrapper: { $path }

journal.cli.setup.clean.journal.line = Journal: { $path }

journal.cli.setup.clean.no.binding = saubere Deinstallation abgelehnt: keine übernommene Installationsidentität

journal.cli.setup.clean.no.owned.path = kein eigener Pfad

journal.cli.setup.clean.outcome.already.complete = die saubere Deinstallation ist bereits abgeschlossen

journal.cli.setup.clean.outcome.cancelled = abgebrochen

journal.cli.setup.clean.outcome.complete = saubere Deinstallation abgeschlossen: { $removed } entfernt, { $already_absent } bereits nicht vorhanden, { $preserved } erhalten, { $skipped } übersprungen, { $failed } fehlgeschlagen

journal.cli.setup.clean.outcome.no.eligible = keine geeigneten Dateien der Ersteinrichtung zum Entfernen

journal.cli.setup.clean.outcome.not.tty = kein tty; mit --yes erneut ausführen, um ohne Rückfrage fortzufahren (abgebrochen)

journal.cli.setup.clean.outcome.service.unavailable = Dienstort nicht verfügbar: { $error }

journal.cli.setup.clean.outcome.stopped.failed = saubere Deinstallation nach einem fehlgeschlagenen Schritt angehalten

journal.cli.setup.clean.preflight.census.error = Erfassung der Installationsidentität ist nicht lesbar: { $error }

journal.cli.setup.clean.preflight.census.unreadable = Erfassung der Installationsidentität ist nicht lesbar

journal.cli.setup.clean.preflight.config.mismatch = das Journal der Konfiguration passt nicht zur Installationsidentität

journal.cli.setup.clean.preflight.failed = Vorprüfung der sauberen Deinstallation fehlgeschlagen: { $error }

journal.cli.setup.clean.preflight.overlap = der Identitätsspeicher überlappt ein geschütztes Journal

journal.cli.setup.clean.preflight.prepared = die Installationsidentität ist vorbereitet

journal.cli.setup.clean.readmit.failed = die Installation konnte nach dem Entfernen ihres Dienstes nicht erneut zugelassen werden: { $error }

journal.cli.setup.clean.reason.manifest.preserved = Journal-Manifest bleibt erhalten

journal.cli.setup.clean.reason.no.owned.path.known = kein eigener Pfad ist bekannt

journal.cli.setup.clean.reason.no.owned.path.platform = kein eigener Pfad auf dieser Plattform

journal.cli.setup.clean.reason.preserved.mark = erhalten: { $mark }

journal.cli.setup.clean.reason.preserved.non.dir = Ziel erhalten, das kein Verzeichnis ist

journal.cli.setup.clean.reason.preserved.symlink = Symlink erhalten

journal.cli.setup.clean.reason.service.guard.mismatch = Dienst- oder Wrapper-Guard passt nicht zu dieser Installation, wird nicht entfernt

journal.cli.setup.clean.reason.skill.foreign = Symlink-Ziel ist fremd

journal.cli.setup.clean.reason.skill.preserved = dein eigener skill-Inhalt bleibt erhalten

journal.cli.setup.clean.reason.skipped.after.failure = der Schritt wurde nach einem früheren Fehler nicht ausgeführt

journal.cli.setup.clean.reason.wrapper.cross.repo = Alias zeigt auf { $target }, wird nicht entfernt

journal.cli.setup.clean.reason.wrapper.dangling = Alias zeigt ins Leere (Ziel { $target } fehlt), wird nicht entfernt

journal.cli.setup.clean.reason.wrapper.foreign = Alias ist kein verwalteter Symlink, wird nicht entfernt

journal.cli.setup.clean.reason.wrapper.guard.mismatch = Wrapper-Guard passt nicht zu dieser Installation, wird nicht entfernt

journal.cli.setup.clean.reason.wrapper.unexpected = unerwarteter Alias-Zustand: { $state }

journal.cli.setup.clean.reason.wrapper.unknown = unbekannt

journal.cli.setup.clean.reason.wrapper.worktree = aus einem git-Worktree wird nicht gehandelt

journal.cli.setup.clean.refusal.incompatible = --clean-uninstall lässt sich nicht mit { $flags } kombinieren

journal.cli.setup.clean.refusal.jsonl = JSONL-Ausgabe wird für --clean-uninstall in dieser Version nicht unterstützt.

journal.cli.setup.clean.refused = saubere Deinstallation abgelehnt: { $reason }

journal.cli.setup.clean.step.running = [Schritt { $step }/{ $total }] Deinstallation von { $name } läuft...

journal.cli.setup.error.home.dir = Ersteinrichtung konnte dein Home-Verzeichnis nicht bestimmen

journal.cli.setup.error.refused.before.mutation = solstone journal setup: vor der Änderung abgelehnt: { $error }

journal.cli.setup.error.sign.in.windows.only = solstone journal setup: --initial-start-at-sign-in gibt es nur unter Windows

journal.cli.setup.host.journal.binding.changed = Journal-Bindung geändert

journal.cli.setup.host.packaged.speech.unavailable = Nachweis für gepacktes Gesprochenes nicht verfügbar

journal.cli.setup.identity.adopted.no.marker = übernommener Datensatz hat keine Übernahme-Markierung

journal.cli.setup.identity.changed.retry = die Installation hat sich geändert, während ihr Journal-Ordner geändert wurde; versuch es noch einmal

journal.cli.setup.identity.executable.unavailable = aktuelle Programmdatei ist nicht verfügbar

journal.cli.setup.identity.finish.recovery.first = schließ die Ersteinrichtung oder die Wiederherstellung ab, bevor du den Journal-Ordner änderst

journal.cli.setup.identity.finish.setup.first = schließ die Ersteinrichtung des Journals ab, bevor du seinen Ordner änderst

journal.cli.setup.identity.foreign.artifacts = vorhandene Wrapper- oder Dienst-Artefakte sind an eine andere Installation gebunden

journal.cli.setup.identity.generation.decimal = Generation ist keine kanonische Dezimalzahl

journal.cli.setup.identity.generation.nonzero = Generation muss ungleich null sein

journal.cli.setup.identity.generation.overflow = Generationsüberlauf

journal.cli.setup.identity.generation.range = Generation liegt außerhalb des Bereichs

journal.cli.setup.identity.guard.duplicate = doppelter Identitäts-Guard { $field }

journal.cli.setup.identity.guard.incomplete = Artefakt-Guard ist keine vollständig passende Identität

journal.cli.setup.identity.guard.invalid = ungültiger Identitäts-Guard { $field }

journal.cli.setup.identity.guard.mismatch = Artefakt-Guard passt nicht zur Identität

journal.cli.setup.identity.guard.missing = Felder des Identitäts-Guards sind unvollständig

journal.cli.setup.identity.guard.root.mismatch = Artefakt-Guard passt nicht zur zugelassenen Wurzel

journal.cli.setup.identity.guarded.no.record = vorhandene Wrapper- oder Dienst-Artefakte haben einen Guard, aber im Identitätsspeicher liegt kein passender Installationsdatensatz

journal.cli.setup.identity.home.unset = HOME ist nicht gesetzt

journal.cli.setup.identity.io.create.home = Home-Verzeichnis der Ersteinrichtung anlegen: { $error }

journal.cli.setup.identity.legacy.manifest.invalid = Nachweis zum älteren Manifest ist fehlerhaft oder nicht lesbar

journal.cli.setup.identity.legacy.needs.manifest = ältere Artefakte brauchen ein gültiges schema-v1-Manifest ohne Anbieter

journal.cli.setup.identity.namespace.mismatch = Namensraum-Datensatz passt nicht zu seinem Wurzel-Namensraum

journal.cli.setup.identity.no.bootstrap.evidence = vorhandene Artefakte haben keinen gültigen Bootstrap-Nachweis

journal.cli.setup.identity.not.found = die Installationsidentität existiert nicht

journal.cli.setup.identity.owner.root.changed = deine Zulassungswurzel hat sich während der Sperre geändert

journal.cli.setup.identity.path.outside = PATH wird außerhalb der V2-Installation aufgelöst

journal.cli.setup.identity.prepared.uninstall = eine vorbereitete Identität kann nicht deinstalliert werden

journal.cli.setup.identity.provider.app.dir = Basis des Installationsanbieters hat kein Anwendungsverzeichnis

journal.cli.setup.identity.provider.parent = Basis des Installationsanbieters hat kein übergeordnetes Verzeichnis

journal.cli.setup.identity.record.stage.malformed = fehlerhafte Datensatz-Stufe

journal.cli.setup.identity.record.stage.stale = veraltete Datensatz-Stufe passt nicht zu ihrem Namen

journal.cli.setup.identity.root.unavailable = Installationswurzel ist nicht verfügbar

journal.cli.setup.identity.tombstoned.no.marker = Tombstone-Datensatz hat keine Übernahme-Markierung

journal.cli.setup.identity.uncertain.artifacts = Artefakt-Bindung ist fehlerhaft oder mehrdeutig

journal.cli.setup.identity.windows.path.odd = Windows-Pfad hat eine ungerade Byte-Anzahl

journal.cli.setup.identity.windows.platform.mismatch = deine Plattform passt nicht zu diesem Windows-Build

journal.cli.setup.identity.wrapper.journal.unsafe = wirksamer Journal-Pfad lässt sich in verwalteten Wrappern nicht sicher darstellen

journal.cli.setup.narrate.artifacts.header = Artefakte:

journal.cli.setup.narrate.artifacts.none = { $indent }keine

journal.cli.setup.narrate.close.finish.unix = Ersteinrichtung abschließen: solstone journal open /init

journal.cli.setup.narrate.close.finish.url = Ersteinrichtung abschließen: { $url }/init

journal.cli.setup.narrate.close.open.unix = dein Journal öffnen: solstone journal open

journal.cli.setup.narrate.close.open.url = dein Journal öffnen: { $url }

journal.cli.setup.narrate.close.skip.service.unix = sobald dein Journal läuft, öffne es mit: solstone journal open

journal.cli.setup.narrate.close.skip.service.url = sobald dein Journal läuft, öffne es unter { $url }

journal.cli.setup.narrate.complete.counts = { $skipped } von { $total } Schritten schon erledigt; { $ran } ausgeführt

journal.cli.setup.narrate.complete.installed = solstone ist installiert.

journal.cli.setup.narrate.complete.set.up = solstone ist eingerichtet.

journal.cli.setup.narrate.model.recovery.direct = die Modellinstallation wurde nicht abgeschlossen. führ { $journal } install-models --variant { $variant } aus, dann { $journal } setup.

journal.cli.setup.narrate.model.recovery.installer = die Modellinstallation wurde nicht abgeschlossen. führ { $journal } install-models --variant { $variant } aus.

journal.cli.setup.narrate.prior.clean.force = solstone journal setup lief zuletzt sauber am { $completed_at }; alle Schritte werden erneut ausgeführt (--force).

journal.cli.setup.narrate.prior.clean.verify = solstone journal setup lief zuletzt sauber am { $completed_at }; der aktuelle Zustand wird geprüft.

journal.cli.setup.narrate.prior.force.hint = Benutze --force, um alle Schritte bedingungslos erneut auszuführen.

journal.cli.setup.narrate.prior.incomplete.header = der letzte Lauf von solstone journal setup am { $started_at } hat diese Schritte unvollständig gelassen:

journal.cli.setup.narrate.prior.incomplete.hint = Ein erneuter Lauf prüft den Zustand und führt unvollständige Schritte erneut aus.

journal.cli.setup.narrate.prior.incomplete.item = { $indent }- { $name } (fehlgeschlagen)

journal.cli.setup.narrate.sol.keeps.journal = solstone auf diesem Mac hält dieses Journal schon, deshalb hat die Ersteinrichtung keinen Hintergrund-Starter installiert.{ $line_break }Führ solstone journal doctor aus, wenn etwas falsch aussieht.

journal.cli.setup.narrate.step.failed = solstone journal setup: { $name } fehlgeschlagen: { $message }

journal.cli.setup.narrate.step.running = [Schritt { $index }/{ $total }] { $name } läuft...

journal.cli.setup.narrate.step.running.command = [Schritt { $index }/{ $total }] { $name } läuft: { $command }

journal.cli.setup.narrate.step.skipped = [Schritt { $index }/{ $total }] { $name } übersprungen: { $reason }

journal.cli.setup.parser.cwd = aktuelles Verzeichnis konnte nicht bestimmt werden: { $error }

journal.cli.setup.parser.path.empty = { $journal_flag } darf nicht leer sein

journal.cli.setup.parser.path.encoding = { $journal_flag } konnte nicht aufgelöst werden: der Pfad ist nicht { $encoding }

journal.cli.setup.parser.path.resolve = { $journal_flag } konnte nicht aufgelöst werden: { $error }

journal.cli.setup.parser.port = { $port_flag } muss in { $range } liegen (war { $value })

journal.cli.setup.parser.sign.in.encoding = Argument { $sign_in_flag }: erwartet { $on_value } oder { $off_value }

journal.cli.setup.parser.sign.in.invalid = Argument { $sign_in_flag }: ungültige Auswahl: { $value } (wähle aus { $choices })

journal.cli.setup.parser.sign.in.repeated = Argument { $sign_in_flag }: darf nicht wiederholt werden

journal.cli.setup.parser.timeout = Argument { $timeout_flag }: ungültiger Ganzzahlwert

journal.cli.setup.parser.unknown = nicht erkannte Argumente

journal.cli.setup.parser.variant = Argument { $variant_flag }: ungültige Auswahl: { $value } (wähle aus { $choices })

journal.cli.setup.path.added = Pfad: ~/.local/bin zum Shell-PATH hinzugefügt

journal.cli.setup.path.added.restart = Pfad: ~/.local/bin zum Shell-PATH hinzugefügt — starte deine Shell neu oder führ 'exec $SHELL -l' aus, damit sie es übernimmt

journal.cli.setup.path.already = Pfad: ~/.local/bin ist schon im PATH

journal.cli.setup.path.manual = Pfad: ~/.local/bin konnte nicht automatisch zum PATH hinzugefügt werden — füge diese Zeile von Hand in deine Shell-rc ein: export PATH="$HOME/.local/bin:$PATH"

journal.cli.setup.path.windows = Windows-Pakete stellen solstone direkt bereit; kein POSIX-PATH-Wrapper wurde installiert.

journal.cli.setup.plan.brain = würde die Spur des lokalen Anbieters setzen, wenn dieser Computer das lokale Modell ausführen kann, und es bootstrappen, wenn nichts es blockiert

journal.cli.setup.plan.doctor = würde ausführen: { $binary } journal doctor --readiness

journal.cli.setup.plan.heading.dry.run = Probelauf der Ersteinrichtung:

journal.cli.setup.plan.heading.explain = Plan der Ersteinrichtung:

journal.cli.setup.plan.identity.admission = Identitätszulassung: würde vor verändernden Schritten der Ersteinrichtung laufen

journal.cli.setup.plan.install.models = würde ausführen: { $binary } journal install-models --variant { $variant }

journal.cli.setup.plan.journal = { $indent }Journal: { $path } ({ $source })

journal.cli.setup.plan.mode = { $indent }Modus: { $mode }

journal.cli.setup.plan.model.download.default = { $indent }Modell-Download: kein Zeitlimit der Ersteinrichtung (Vorgabe)

journal.cli.setup.plan.port = { $indent }Port: { $value } ({ $source })

journal.cli.setup.plan.service = würde den Journal-Dienst installieren und starten

journal.cli.setup.plan.skills.journal = würde ausführen: { $binary } skills install --project { $journal } --agent all

journal.cli.setup.plan.skills.user = würde ausführen: { $binary } skills install --agent all

journal.cli.setup.plan.source.checkout.false = { $indent }Quell-Checkout: False

journal.cli.setup.plan.source.checkout.true = { $indent }Quell-Checkout: True

journal.cli.setup.plan.step.header = [Schritt { $index }/{ $total }] { $name }

journal.cli.setup.plan.step.timeout = { $indent }Zeitlimit des Schritts in Sekunden: { $value } ({ $source })

journal.cli.setup.plan.variant = { $indent }Variante: { $value } ({ $source })

journal.cli.setup.plan.wrapper.unix = würde verwaltete Wrapper für solstone und journal im Prozess bereitstellen

journal.cli.setup.plan.wrapper.windows = würde die Bereitstellung von POSIX-Wrappern überspringen; das Windows-Paket stellt die Befehle direkt bereit

journal.cli.setup.plan.write.config = würde schreiben: { $path }

journal.cli.setup.prompt.existing.journal = { $path } enthält schon Journaldaten; fortfahren? [y/N]:

journal.cli.setup.prompt.proceed = fortfahren? [y/N]:

journal.cli.setup.service.restart.exited = Neustart des Dienstes beendet mit { $code }

journal.cli.setup.warning.launcher.ownership = Warnung: Zugehörigkeit des solstone-Starters konnte nicht geprüft werden: { $error }

journal.cli.setup.warning.manifest = Warnung: Manifest der Ersteinrichtung konnte nicht geschrieben werden: { $error }

journal.cli.setup.warning.package.receipt = solstone journal setup: Warnung: Paketbeleg wurde nicht geschrieben: { $error }

journal.cli.setup.wrapper.retired.note = die Befehlszeilenwerkzeuge der älteren Version wurden entfernt; ihre Sicherungen liegen in ~/.local/share/solstone/setup-backups

journal.cli.setup.wrapper.skip.path = die Login-Dateien der Shell wurden unverändert gelassen, weil --skip-path gewählt wurde

journal.cli.think.activity.blocked = Aktivität blockiert

journal.cli.think.activity.body.data = diese Aktivität stammt von einem { $provider }-Training, und das sind Körperdaten; darauf läuft nichts weiter.

journal.cli.think.activity.day.missing = Tagesdatei der Aktivität fehlt: { $path }

journal.cli.think.activity.fingerprint.failed = Fingerabdruck der Aktivitätseingaben kann nicht erstellt werden

journal.cli.think.activity.malformed = die Facetten-Deklaration ist fehlerhaft. korrigiere sie und versuch es dann erneut mit { $reactivate_flag }.

journal.cli.think.activity.missing = die Zielfacette fehlt. nach der Wiederherstellung ihrer Deklaration versuch es mit { $reactivate_flag }; sonst ordne das Quellsegment neu ein.

journal.cli.think.activity.muted.blocked = Zielfacette ist stummgeschaltet. hebe die Stummschaltung auf und versuch die blockierte Aktivität erneut mit { $reactivate_flag }.

journal.cli.think.activity.muted.retry = Zielfacette ist stummgeschaltet. hebe die Stummschaltung auf, um die unfertige Aktivitätsverarbeitung fortzusetzen.

journal.cli.think.activity.owner.field.held = Aktivitätsausgabe ist von dir zurückgehalten

journal.cli.think.activity.pending = erneuter Aktivitätsversuch ausstehend

journal.cli.think.activity.read.blocked = Facetten-Deklaration konnte nicht gelesen werden. stell den Zugriff wieder her und versuch die blockierte Aktivität erneut mit { $reactivate_flag }.

journal.cli.think.activity.read.retry = die Facetten-Deklaration konnte nicht gelesen werden. die unfertige Aktivität wird automatisch erneut versucht.

journal.cli.think.activity.record.missing = für diese Einordnung gibt es keinen Aktivitätsdatensatz. nach der Korrektur der Deklaration verarbeite das Quellsegment erneut.

journal.cli.think.activity.repair.required = erfordert Reparatur

journal.cli.think.activity.replaced = die Zielfacette wurde ersetzt. ordne das Quellsegment neu ein; ein altes Ergebnis kann nicht auf den Ersatz angewendet werden.

journal.cli.think.activity.retry.identity = Identität der Aktivitätsarbeit stimmt nicht überein

journal.cli.think.activity.retry.kind = keine Aktivitätsarbeit

journal.cli.think.activity.retry.muted = die Facette der Aktivität ist stummgeschaltet

journal.cli.think.activity.routing = korrigiere die Deklaration und verarbeite dann dieses Quellsegment erneut.

journal.cli.think.activity.row.missing = Aktivitätszeile fehlt: { $path }

journal.cli.think.activity.settled = { $facet }/{ $activity } festgeschrieben: diese Aktivität stammt von einem { $provider }-Training, und das sind Körperdaten; darauf läuft nichts weiter.

journal.cli.think.activity.state = Aktivitätszustand nicht gespeichert: { $error }

journal.cli.think.activity.state.segment = { $segment }: Aktivitätszustand nicht gespeichert: { $error }

journal.cli.think.activity.work.malformed = fehlerhafte Aktivitätsarbeit: { $path }

journal.cli.think.activity.work.pending = Aktivitätsarbeit ausstehend: { $failures }

journal.cli.think.activity.work.running = die vorherige Aktivitätseingabe läuft noch

journal.cli.think.activity.work.unreadable = Aktivitätsarbeit { $path } kann nicht gelesen werden: { $error }

journal.cli.think.activity.work.version = nicht unterstützte Version der Aktivitätsarbeit

journal.cli.think.argument.integer = Argument { $option }: ungültiger int-Wert

journal.cli.think.argument.unknown = nicht erkannte Argumente: { $argument }

journal.cli.think.argument.value = Argument { $option }: ein Argument erwartet

journal.cli.think.cancelled = Denken abgebrochen

journal.cli.think.daily.blockers.more = { $names }, +{ $count } mehr

journal.cli.think.daily.incomplete = { $units ->
    [one] { $blockers ->
        [one] { $phase } ({ $units } Tageseinheit nicht abgeschlossen; { $blockers } Segmentblocker bleibt{ $details })
        *[other] { $phase } ({ $units } Tageseinheit nicht abgeschlossen; { $blockers } Segmentblocker bleiben{ $details })
        }
    *[other] { $blockers ->
        [one] { $phase } ({ $units } Tageseinheiten nicht abgeschlossen; { $blockers } Segmentblocker bleibt{ $details })
        *[other] { $phase } ({ $units } Tageseinheiten nicht abgeschlossen; { $blockers } Segmentblocker bleiben{ $details })
        }
    }

journal.cli.think.daily.incomplete.record = { $phase } ({ $units } Tageseinheiten nicht abgeschlossen; { $blockers } Segmentblocker bleiben{ $details })

journal.cli.think.daily.inputs.changed = Roheingabe hat sich während der ganztägigen Verarbeitung geändert

journal.cli.think.daily.location.packet = Wiederverwendung des Tagesorts hat kein gespeichertes Paket

journal.cli.think.daily.location.read = Tagesort konnte nicht gelesen werden

journal.cli.think.daily.snapshot.changed = Tagesnachweis hat sich vor der Vorbereitung geändert

journal.cli.think.daily.snapshot.changed.after = Tagesnachweis oder Kontrakt hat sich während der Vorbereitung geändert

journal.cli.think.daily.snapshot.freeze = Vorbereitung der Tageseingabe: { $error }

journal.cli.think.daily.snapshot.parent = Talent-Wurzel hat kein übergeordnetes Verzeichnis

journal.cli.think.daily.snapshot.removed = Tages-Talent wurde während der Vorbereitung entfernt

journal.cli.think.error = { $command }: { $message }

journal.cli.think.error.with.log = { $primary }; { $log }

journal.cli.think.health.day.invalid = ungültiger Tag { $day }; erwartet { $date_format }

journal.cli.think.health.directory = Verzeichnis { $path } kann nicht gelesen werden: { $detail }

journal.cli.think.health.marker = Fehler des Zustands-Markers: { $detail }

journal.cli.think.health.marker.atomic = Zustands-Marker konnte nicht veröffentlicht werden: { $error }

journal.cli.think.health.marker.fingerprint = Fingerabdruck des Zustands-Markers konnte nicht gelesen werden: { $detail }

journal.cli.think.health.marker.io = E/A des Zustands-Markers bei { $path } fehlgeschlagen: { $error }

journal.cli.think.health.marker.lock = Sperre des Zustands-Markers fehlgeschlagen: { $error }

journal.cli.think.health.marker.malformed = Zustands-Marker ist fehlerhaft: { $path }

journal.cli.think.health.marker.stream = Zustands-Marker des Datenstroms ist fehlerhaft

journal.cli.think.health.metadata = Dateimetadaten { $path } können nicht gelesen werden: { $detail }

journal.cli.think.health.progress.unreadable = Segmentfortschritt enthält unlesbare Datensätze

journal.cli.think.health.read = E/A-Fehler des Journals: { $detail }

journal.cli.think.health.source = Fehler der Zustandsprotokoll-Quelle: { $detail }

journal.cli.think.help = Aufruf: { $syntax }

journal.cli.think.host.sense.missing = gehosteter Sense-Executor ist nicht verfügbar

journal.cli.think.intent.gone = destruktive Anfrage wartet nicht mehr

journal.cli.think.log.activity.join = { $command }: verspätetes { $day }/{ $segment } hat sich seiner offenen Aktivität angeschlossen

journal.cli.think.log.activity.no.advance = { $command }: { $day }/{ $segment } bringt den laufenden Aktivitätszustand nicht voran

journal.cli.think.log.activity.replay = { $command }: verspätetes { $day }/{ $segment } wird mit dem Tag seines Datenstroms erneut abgespielt

journal.cli.think.log.activity.turn = Aktivitätszustand von { $stream } wird ohne seinen Zug fortgeschrieben: { $error }

journal.cli.think.log.budget = Denkergebnis überschreitet das Datensatzbudget des Betriebsprotokolls

journal.cli.think.log.cadence.read = Kadenzzustand konnte nicht geladen werden: { $error }

journal.cli.think.log.cadence.set.aside = Kadenzzustand konnte nicht geladen werden: beschädigtes Artefakt beiseitegelegt

journal.cli.think.log.daily.search = Sucharbeit konnte vor dem täglichen Denken nicht untersucht werden: { $error }

journal.cli.think.log.day.format = Tag muss { $date_format } sein

journal.cli.think.log.day.local = ungültiger lokaler Tag

journal.cli.think.log.display.metadata = Anzeigemetadaten für { $path } konnten nicht gespeichert werden: { $error }

journal.cli.think.log.failed = Denk-Laufprotokoll { $path } fehlgeschlagen: { $detail }

journal.cli.think.log.failure.lock = Fehlersperre des Denk-Laufprotokolls ist vergiftet

journal.cli.think.log.held.release = { $name }, zurückgehalten, konnte für { $segment } nicht freigegeben werden: { $error }

journal.cli.think.log.lock = Sperre des Denk-Laufprotokolls ist vergiftet

journal.cli.think.log.name.answers = Namensantworten: { $answers } geändert, { $judged } Aktivitäten erneut beurteilt, { $changed } geändert, { $waiting } wartend

journal.cli.think.log.provider = { $left ->
    [one] der Anbieter fürs Denken ist nicht verfügbar; { $left } Segment wartet auf den nächsten Lauf
    *[other] der Anbieter fürs Denken ist nicht verfügbar; { $left } Segmente warten auf den nächsten Lauf
    }

journal.cli.think.log.provider.record = der Anbieter fürs Denken ist nicht verfügbar; { $left } Segmente warten auf den nächsten Lauf

journal.cli.think.log.reconciliation = Abstimmung des erneuten Versuchs fehlgeschlagen: { $error }

journal.cli.think.log.segment.turn = Denken für { $day }/{ $segment } ohne seinen Zug: { $error }

journal.cli.think.markdown.entities = Sense-Entitäten

journal.cli.think.markdown.entity = { $kind } — { $name } (Rolle={ $role }, Quelle={ $source }) — { $context }

journal.cli.think.markdown.object.both = { $frames ->
    [one] { $photos ->
        [one] { $class }: in { $frames } Bildschirmbild und { $photos } Foto
        *[other] { $class }: in { $frames } Bildschirmbild und { $photos } Fotos
        }
    *[other] { $photos ->
        [one] { $class }: in { $frames } Bildschirmbildern und { $photos } Foto
        *[other] { $class }: in { $frames } Bildschirmbildern und { $photos } Fotos
        }
    }

journal.cli.think.markdown.object.both.maximum = { $frames ->
    [one] { $photos ->
        [one] { $class }: in { $frames } Bildschirmbild und { $photos } Foto, bis zu { $most } auf einmal
        *[other] { $class }: in { $frames } Bildschirmbild und { $photos } Fotos, bis zu { $most } auf einmal
        }
    *[other] { $photos ->
        [one] { $class }: in { $frames } Bildschirmbildern und { $photos } Foto, bis zu { $most } auf einmal
        *[other] { $class }: in { $frames } Bildschirmbildern und { $photos } Fotos, bis zu { $most } auf einmal
        }
    }

journal.cli.think.markdown.object.frames = { $frames ->
    [one] { $class }: in { $frames } Bildschirmbild
    *[other] { $class }: in { $frames } Bildschirmbildern
    }

journal.cli.think.markdown.object.frames.maximum = { $frames ->
    [one] { $class }: in { $frames } Bildschirmbild, bis zu { $most } auf einmal
    *[other] { $class }: in { $frames } Bildschirmbildern, bis zu { $most } auf einmal
    }

journal.cli.think.markdown.object.photos = { $photos ->
    [one] { $class }: in { $photos } Foto
    *[other] { $class }: in { $photos } Fotos
    }

journal.cli.think.markdown.object.photos.maximum = { $photos ->
    [one] { $class }: in { $photos } Foto, bis zu { $most } auf einmal
    *[other] { $class }: in { $photos } Fotos, bis zu { $most } auf einmal
    }

journal.cli.think.markdown.objects = Objekte

journal.cli.think.markdown.partial.counts = Ausgelassen: { $details }

journal.cli.think.markdown.partial.input = Ausgelassen: Ein Teil der Eingabe passte nicht. Dieses Ergebnis wurde aus einem Teil der Eingabe geschrieben.

journal.cli.think.markdown.partial.story = Diese Geschichte und alles mit ihr Gespeicherte wurde aus einem Teil der Aktivität geschrieben, nicht aus der ganzen.

journal.cli.think.markdown.quotes = Zitate

journal.cli.think.markdown.sense.entities = ### Entitäten

journal.cli.think.markdown.sense.facets = ### Facetten

journal.cli.think.markdown.sense.record = ## Sense — { $kind }

journal.cli.think.markdown.sense.speakers = **Sprecher:** { $names }

journal.cli.think.markdown.speaker = Sprecher { $number }

journal.cli.think.markdown.terminal.clock = { $clock } · Terminal

journal.cli.think.markdown.terminal.notice = Text, in deinem Terminal angezeigt. das sagt, was sichtbar war, nicht was du gelesen, getippt oder getan hast. Passwörter und andere Geheimnisse zu maskieren ist nach bestem Bemühen; deine Schwärzungsregeln gelten nicht.

journal.cli.think.markdown.topics = Themen

journal.cli.think.outcome.completed = { $command }: { $count } abgeschlossen

journal.cli.think.outcome.failed = { $command }: { $count } fehlgeschlagen

journal.cli.think.phase.daily.prerequisites = die tägliche Arbeit wartet auf ihre Segmentvoraussetzungen

journal.cli.think.phase.exit = Exit { $code }

journal.cli.think.phase.segment.prerequisites = Segmentvoraussetzungen haben sich geändert

journal.cli.think.plan.activity.count = Segmente: { $segments }

journal.cli.think.plan.activity.empty = Keine Agenten passen zum Aktivitätstyp '{ $kind }'

journal.cli.think.plan.activity.heading = { $operation ->
    [refresh] Tag { $day } { $activity_flag } { $activity } { $facet_flag } { $facet } (Aktualisierung)
    *[ordinary] Tag { $day } { $activity_flag } { $activity } { $facet_flag } { $facet }
    }

journal.cli.think.plan.activity.missing = Aktivität nicht gefunden: { $activity } in der Facette '{ $facet }' am { $day }

journal.cli.think.plan.activity.type = Typ:     { $kind }

journal.cli.think.plan.cadence = Tag { $day } — Kadenzagenten

journal.cli.think.plan.cadence.fire = { $segments ->
    [one] { $activities ->
        [one] Auslösung  { $name } — Fenster: { $segments } Segment, { $activities } Aktivität
        *[other] Auslösung  { $name } — Fenster: { $segments } Segment, { $activities } Aktivitäten
        }
    *[other] { $activities ->
        [one] Auslösung  { $name } — Fenster: { $segments } Segmente, { $activities } Aktivität
        *[other] Auslösung  { $name } — Fenster: { $segments } Segmente, { $activities } Aktivitäten
        }
    }

journal.cli.think.plan.cadence.fire.record = Auslösung  { $name } — Fenster: { $segments } Segmente, { $activities } Aktivitäten

journal.cli.think.plan.cadence.idle = no-op { $name } — keine neue Arbeit seit dem letzten Kadenzlauf

journal.cli.think.plan.cadence.skip = überspringen  { $name } — Intervall noch nicht abgelaufen ({ $seconds } s < { $minutes } min)

journal.cli.think.plan.day = Tag { $day }

journal.cli.think.plan.day.refresh = Tag { $day } (Aktualisierung)

journal.cli.think.plan.day.segment = Tag { $day } Segment { $segment }

journal.cli.think.plan.day.segment.refresh = Tag { $day } Segment { $segment } (Aktualisierung)

journal.cli.think.plan.empty.cadence = Keine Prompts für den Zeitplan: { $cadence }

journal.cli.think.plan.empty.weekly = Keine Prompts für den Zeitplan: { $weekly }

journal.cli.think.plan.flush = Tag { $day } { $flush_flag } Segment { $segment }

journal.cli.think.plan.flush.empty = Keine für die Leerung geeigneten Agenten

journal.cli.think.plan.flush.talent = { $kind ->
    [generate] Gen  { $name }
    *[agent] Agent  { $name }
    }

journal.cli.think.plan.idle = Segmente im Leerlauf: nur Stubs schreiben (außer bei { $refresh_flag })

journal.cli.think.plan.orchestrator = Sense-Orchestrator (linear):

journal.cli.think.plan.post.indexer = Nachphase: { $command }

journal.cli.think.plan.post.stats = Nachphase: { $command }

journal.cli.think.plan.pre = Vorphase:  { $command } { $day } { $jobs_flag } { $jobs }

journal.cli.think.plan.priority = Priorität { $priority }:

journal.cli.think.plan.reprocess = { $count ->
    [one] Tag { $day } — { $count } Segment erneut verarbeiten
    *[other] Tag { $day } — { $count } Segmente erneut verarbeiten
    }

journal.cli.think.plan.reprocess.record = Tag { $day } — { $count } Segmente erneut verarbeiten

journal.cli.think.plan.schedule.empty = Keine Prompts für den Zeitplan: { $schedule }

journal.cli.think.plan.segment.row = [{ $index }/{ $total }] { $segment } ({ $start }-{ $end }) Datenstrom={ $stream }

journal.cli.think.plan.skip = überspringen { $name } — keine Aktivität: { $facets }

journal.cli.think.plan.state = Zustandsmaschine der Aktivität: Aktualisierungen je Segment

journal.cli.think.plan.step.entities = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (vorhanden) — immer, wenn nicht im Leerlauf
    [new] { $step }. { $name } ({ $kind }/{ $format }) (neu) — immer, wenn nicht im Leerlauf
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — immer, wenn nicht im Leerlauf
    }

journal.cli.think.plan.step.possible = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (vorhanden) — bei einem Transkript von Gesprochenem (sperrt das Segment nie)
    [new] { $step }. { $name } ({ $kind }/{ $format }) (neu) — bei einem Transkript von Gesprochenem (sperrt das Segment nie)
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — bei einem Transkript von Gesprochenem (sperrt das Segment nie)
    }

journal.cli.think.plan.step.screen = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (vorhanden) — wenn { $recommend_screen_record }
    [new] { $step }. { $name } ({ $kind }/{ $format }) (neu) — wenn { $recommend_screen_record }
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — wenn { $recommend_screen_record }
    }

journal.cli.think.plan.step.sense = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (vorhanden) — verpflichtend
    [new] { $step }. { $name } ({ $kind }/{ $format }) (neu) — verpflichtend
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — verpflichtend
    }

journal.cli.think.plan.step.speaker.attribution = { $presence ->
    [existing] { $step }. { $name } ({ $kind }/{ $format }) (vorhanden) — bei Audio-Einbettungen (die Modellebene nur, wenn { $recommend_speaker_attribution })
    [new] { $step }. { $name } ({ $kind }/{ $format }) (neu) — bei Audio-Einbettungen (die Modellebene nur, wenn { $recommend_speaker_attribution })
    *[plain] { $step }. { $name } ({ $kind }/{ $format }) — bei Audio-Einbettungen (die Modellebene nur, wenn { $recommend_speaker_attribution })
    }

journal.cli.think.plan.table.facet = { $row ->
    [generator_existing] Gen  { $name }/{ $facet } (vorhanden)
    [generator_new] Gen  { $name }/{ $facet } (neu)
    [generator_plain] Gen  { $name }/{ $facet }
    [agent_existing] Agent  { $name }/{ $facet } (vorhanden)
    [agent_new] Agent  { $name }/{ $facet } (neu)
    *[agent_plain] Agent  { $name }/{ $facet }
    }

journal.cli.think.plan.table.talent = { $row ->
    [generator_existing] Gen  { $name } (vorhanden)
    [generator_new] Gen  { $name } (neu)
    [generator_plain] Gen  { $name }
    [agent_existing] Agent  { $name } (vorhanden)
    [agent_new] Agent  { $name } (neu)
    *[agent_plain] Agent  { $name }
    }

journal.cli.think.plan.total = { $total ->
    [one] Gesamt: { $total } Agent
    *[other] Gesamt: { $total } Agenten
    }

journal.cli.think.plan.total.record = Gesamt: { $total } Agenten

journal.cli.think.plan.weekly = Tag { $day } — Wochenagenten

journal.cli.think.preflight.disk = Dein Journal hat auf seiner Platte keinen Platz mehr ({ $detail }). Reparatur: gib auf dieser Platte Platz frei und starte dein Journal dann erneut.

journal.cli.think.preflight.held = { $reason }: { $role_key }{ $role } { $pid_key }{ $pid } { $time_key }{ $started } { $generation_key }{ $generation } { $age_key }{ $age }; nutze den betreuten Pfad oder beende diesen Prozess sauber

journal.cli.think.preflight.held.unavailable = { $reason }: { $owner }; nutze den betreuten Pfad oder beende den aktuellen Prozess sauber

journal.cli.think.priority.environment = ungültige Arbeitspriorität: { $error }

journal.cli.think.priority.syntax = ungültige Arbeitspriorität; Band 1–6 und eine Quellzeit erwartet

journal.cli.think.repair.segment.missing = Reparatursegment fehlt

journal.cli.think.reprocess.record = Ergebnis erneuter Verarbeitung konnte nicht aufgezeichnet werden: { $error }

journal.cli.think.retry.activity.changed = Berechtigung zum erneuten Aktivitätsversuch geändert

journal.cli.think.retry.activity.released = Aktivität ist nicht mehr beiseitegelegt

journal.cli.think.retry.daily.changed = Berechtigung zum erneuten Tagesversuch geändert

journal.cli.think.retry.daily.missing = Tagesberechtigung fehlt

journal.cli.think.retry.daily.released = Tageseinheit ist nicht mehr zurückgehalten

journal.cli.think.retry.daily.terminal.missing = täglicher Abschlussdatensatz fehlt

journal.cli.think.retry.daily.unresolved = erneuter Tagesversuch ist noch ungeklärt

journal.cli.think.retry.daily.unused = Tagesschreiber hat die Berechtigung zum erneuten Versuch nicht verbraucht

journal.cli.think.retry.day.mismatch = Versuchstag stimmt nicht überein

journal.cli.think.retry.evidence.missing = Versuchsnachweis fehlt

journal.cli.think.retry.permission.changed = Berechtigung zum erneuten Versuch geändert

journal.cli.think.retry.raw.coordinate.invalid = ungültige native Versuchskoordinate

journal.cli.think.retry.raw.coordinate.utf8 = Versuchskoordinate ist nicht UTF8

journal.cli.think.retry.raw.fingerprint.invalid = Fingerabdruck der nativen Fehlereingabe ist fehlerhaft

journal.cli.think.retry.raw.handler.missing = nativer Fehler hat keinen Handler

journal.cli.think.retry.raw.header.incomplete = nativer Verarbeitungskopf ist unvollständig

journal.cli.think.retry.raw.header.kind = nativer Verarbeitungskopf ist kein Objekt

journal.cli.think.retry.raw.kind = keine native Analyse

journal.cli.think.retry.raw.parent.missing = native Ausgabe hat kein übergeordnetes Verzeichnis

journal.cli.think.retry.raw.permission.unused = nativer Schreiber hat die Berechtigung zum erneuten Versuch nicht verbraucht

journal.cli.think.retry.raw.receipt.changed = nativer Abschlussbeleg geändert

journal.cli.think.retry.raw.receipt.missing = nativer Abschlussbeleg fehlt

journal.cli.think.retry.raw.receipt.unpublished = nativer Schreiber hat den Abschlussbeleg dieses erneuten Versuchs nicht veröffentlicht

journal.cli.think.retry.raw.receipt.unreadable = nativer Abschlussbeleg ist nicht lesbar

journal.cli.think.retry.raw.record.malformed = nativer Verarbeitungsdatensatz ist fehlerhaft

journal.cli.think.retry.raw.released = native Ausgabe ist nicht mehr zurückgehalten

journal.cli.think.retry.raw.schema.unknown = natives Verarbeitungsschema wird nicht erkannt

journal.cli.think.retry.raw.source.invalid = nativer Fehler hat keinen sicheren Quelldateinamen

journal.cli.think.retry.raw.source.mismatch = native Fehlerquelle stimmt nicht mit ihrer Ausgabe überein

journal.cli.think.retry.segment.changed = Berechtigung zum erneuten Segmentversuch geändert

journal.cli.think.retry.talent.disabled = zurückgehaltenes Talent ist nicht mehr aktiviert

journal.cli.think.retry.talent.dispatch = zurückgehaltenes Talent konnte nicht zugeteilt werden

journal.cli.think.retry.talent.segment.missing = Segment existiert nicht mehr

journal.cli.think.retry.talent.skipped = zurückgehaltenes Talent konnte nicht erneut versucht werden

journal.cli.think.routing.log.malformed = Weiterleitungswiederherstellung ist aus fehlerhaften Zustandsprotokollen nicht lesbar

journal.cli.think.routing.source.invalid = zurückgehaltene Weiterleitungsquelle ist ungültig

journal.cli.think.segment.ambiguous = { $count ->
    [one] mehrdeutiges Segment { $day }/{ $segment }: { $count } Datenstrom teilt sich diesen Namen
   *[other] mehrdeutiges Segment { $day }/{ $segment }: { $count } Datenströme teilen sich diesen Namen
    }

journal.cli.think.sense.complete = Stapelverarbeitung abgeschlossen

journal.cli.think.sense.config.concurrency = sense: ungültig { $handler }.{ $setting }; Vorgabe ist 1

journal.cli.think.sense.config.read = sense: Journal-Konfiguration nicht verfügbar: { $error }

journal.cli.think.sense.config.runtime = sense: ungültig { $handler }.{ $setting }; Vorgabe ist { $seconds } s

journal.cli.think.sense.conflict = { $sense_batch_flag } ist nicht vereinbar mit { $options }

journal.cli.think.sense.day = { $sense_batch_flag } erfordert { $day_flag }

journal.cli.think.sense.day.missing = Tagesverzeichnis nicht gefunden: { $path }

journal.cli.think.sense.deferred = { $quantity ->
    [one] Stapelverarbeitung zurückgestellt: { $deferred } von { $ran } Handler wartet; unfertige Dateien werden erneut versucht
    *[other] Stapelverarbeitung zurückgestellt: { $deferred } von { $ran } Handlern warten; unfertige Dateien werden erneut versucht
    }

journal.cli.think.sense.dry.empty = Keine unverarbeiteten Dateien gefunden

journal.cli.think.sense.dry.would.process = { $quantity ->
    [one] würde { $count } Datei verarbeiten ({ $breakdown }):
    *[other] würde { $count } Dateien verarbeiten ({ $breakdown }):
    }

journal.cli.think.sense.error.delete = { $path } konnte nicht gelöscht werden: { $error }

journal.cli.think.sense.error.deleted.dirty = erneute Verarbeitung entfernte { $path }; Tag { $day } konnte nicht dauerhaft als geändert markiert werden: { $detail }

journal.cli.think.sense.error.failed = { $failed } fehlgeschlagen von { $ran } gelaufen

journal.cli.think.sense.error.held = sense-Stapel-Lease für Tag { $day } wird von einem anderen Prozess gehalten

journal.cli.think.sense.error.join = Sense-Worker wurde nicht sauber beendet

journal.cli.think.sense.error.lease = Fehler der sense-Stapel-Lease: { $error }

journal.cli.think.sense.error.runtime = Stapel-callosum-Laufzeitumgebung nicht verfügbar

journal.cli.think.sense.error.scan = { $path } konnte nicht durchsucht werden: { $error }

journal.cli.think.sense.error.timeout = Zeitüberschreitung des sense-Stapels nach { $duration }

journal.cli.think.sense.files = { $quantity ->
    [one] Gefunden { $count } unverarbeitete Datei zum Verarbeiten ({ $extensions })
    *[other] Gefunden { $count } unverarbeitete Dateien zum Verarbeiten ({ $extensions })
    }

journal.cli.think.sense.interrupted.write = { $event }-Ereignis konnte nicht in { $path } aufgezeichnet werden: { $error }

journal.cli.think.sense.lease.skip = Sense-Stapel-Lease für Tag { $day } ist gehalten; Stapeldurchlauf wird übersprungen

journal.cli.think.sense.nothing = Keine unverarbeiteten Dateien in { $path } gefunden

journal.cli.think.sense.priority.encoding = { $variable } enthält ungültiges UTF-8

journal.cli.think.sense.processing.day = { $quantity ->
    [one] Verarbeitung der Dateien von Tag { $day } mit { $jobs } gleichzeitigem Auftrag
    *[other] Verarbeitung der Dateien von Tag { $day } mit { $jobs } gleichzeitigen Aufträgen
    }

journal.cli.think.sense.processing.segment = { $quantity ->
    [one] Verarbeitung der Dateien von Tag { $day } (Segment: { $segment }) mit { $jobs } gleichzeitigem Auftrag
    *[other] Verarbeitung der Dateien von Tag { $day } (Segment: { $segment }) mit { $jobs } gleichzeitigen Aufträgen
    }

journal.cli.think.sense.refused = sense-Stapel abgelehnt: { $reason }

journal.cli.think.sense.reprocess.deleted.count = { $quantity ->
    [one] gelöscht { $count } Ausgabedatei
    *[other] gelöscht { $count } Ausgabedateien
    }

journal.cli.think.sense.reprocess.deleted.file = gelöscht: { $path }

journal.cli.think.sense.reprocess.dry.empty = keine Dateien zum Löschen

journal.cli.think.sense.reprocess.dry.would.delete = { $quantity ->
    [one] würde { $count } Ausgabedatei löschen:
    *[other] würde { $count } Ausgabedateien löschen:
    }

journal.cli.think.sense.reprocess.kept.before.voice.only = { $quantity ->
    [one] behalten { $count } Transkript, das erstellt wurde, bevor "andere Stimmen auslassen" an war: schalte es aus, um dieses Transkript erneut zu verarbeiten
    *[other] behalten { $count } Transkripte, die erstellt wurden, bevor "andere Stimmen auslassen" an war: schalte es aus, um sie erneut zu verarbeiten
    }

journal.cli.think.sense.reprocess.kept.voice.only = { $quantity ->
    [one] behalten { $count } Ausgabedatei, die erstellt wurde, während "andere Stimmen auslassen" an war: sie wird nicht erneut verarbeitet
    *[other] behalten { $count } Ausgabedateien, die erstellt wurden, während "andere Stimmen auslassen" an war: sie werden nicht erneut verarbeitet
    }

journal.cli.think.sense.retry.changed = native Versuchseingaben geändert

journal.cli.think.sense.retry.handler = unbekannter nativer Versuchshandler

journal.cli.think.sense.retry.kind = erneuter Versuch ist keine native Analyse

journal.cli.think.sense.retry.mismatch = nativer Fehlerhandler stimmt nicht mit seiner Quelle überein

journal.cli.think.sense.worker.authority = Startberechtigung von { $handler } wurde nicht verwaltet

journal.cli.think.sense.worker.deferred = zurückgestellt (Exit { $code })

journal.cli.think.sense.worker.detail = sense: Tag={ $day } Datenstrom={ $stream } Segment={ $segment } Datei={ $file } Fehler={ $error }

journal.cli.think.sense.worker.exit = { $handler } Exit { $code }

journal.cli.think.sense.worker.failed = das ist nicht gelungen (Exit { $code })

journal.cli.think.sense.worker.launch = Erzeugen von { $handler } fehlgeschlagen

journal.cli.think.sense.worker.launch.failed = sense: Start von { $handler } fehlgeschlagen: { $error }

journal.cli.think.sense.worker.log.tail = sense: Handler-Diagnose Protokoll={ $log } Ende={ $tail }

journal.cli.think.sense.worker.log.unreadable = sense: Handler-Diagnose Protokoll={ $log } nicht lesbar: { $error }

journal.cli.think.sense.worker.output.incomplete = { $handler } endete mit 0 mit unfertiger Ausgabe (Analyseausgabe unvollständig): { $path }

journal.cli.think.sense.worker.output.nonregular = { $handler } endete mit 0 ohne reguläre Ausgabe: { $path }

journal.cli.think.sense.worker.output.unfinished = { $handler } endete mit 0 mit unfertiger Ausgabe ({ $reason }): { $path }

journal.cli.think.sense.worker.output.unreadable = { $handler } endete mit 0 ohne lesbare Ausgabe { $path }: { $error }

journal.cli.think.sense.worker.pool = Pool von { $handler } nicht verfügbar

journal.cli.think.sense.worker.program.executable = { $handler }-Dispatcher: current_exe fehlgeschlagen: { $error }

journal.cli.think.sense.worker.program.missing = { $handler }-Dispatcher fehlt: { $path }

journal.cli.think.sense.worker.program.nonexecutable = { $handler }-Dispatcher ist nicht ausführbar: { $path }

journal.cli.think.sense.worker.program.nonregular = { $handler }-Dispatcher ist keine Datei: { $path }

journal.cli.think.sense.worker.status = sense: Handler={ $handler } Datei={ $file } Ergebnis={ $outcome } Protokoll={ $log }

journal.cli.think.sense.worker.survivors = sense: Prozessbaum nicht eingesammelt Handler={ $handler } Grund={ $reason } verbleibend={ $count }

journal.cli.think.sense.worker.termination = sense: verwaltete Beendigung fehlgeschlagen Handler={ $handler }: { $error }

journal.cli.think.sense.worker.timeout = { $handler } watchdog_timeout nach { $seconds } s

journal.cli.think.sense.worker.watchdog = Watchdog- oder Prozesswarten fehlgeschlagen

journal.cli.think.settle.save = Festschreibungszustand der Aktivität nicht gespeichert: { $error }

journal.cli.think.updated.capability = Aufholfähigkeit auf dieser Plattform nicht verfügbar

journal.cli.think.updated.conflict = { $updated_flag } ist nicht vereinbar mit { $options }

journal.cli.think.updated.io = Aufholen-E/A bei { $path } fehlgeschlagen: { $error }

journal.cli.think.updated.marker = Fehler des Aufhol-Markers: { $error }

journal.cli.think.updated.path = Fehler im Journal-Pfad fürs Aufholen: { $error }

journal.cli.think.updated.state = Aufholzustand ist fehlerhaft: { $detail }

journal.cli.think.usage.error = Aufruf: { $syntax }{ $line_break }{ $command }: Fehler: { $message }

journal.cli.think.validation.activity.incompatible = { $activity_flag } ist nicht vereinbar mit { $segment_flag }, { $segments_flag } und { $flush_flag }

journal.cli.think.validation.activity.requires.day = { $activity_flag } erfordert { $day_flag }

journal.cli.think.validation.activity.requires.facet = { $activity_flag } erfordert { $facet_flag }

journal.cli.think.validation.cadence.incompatible = { $cadence_flag } ist nicht vereinbar mit { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag } und { $weekly_flag }

journal.cli.think.validation.daily.only = { $daily_only_flag } erfordert { $day_flag } und lässt sich nicht mit einem anderen Denkmodus kombinieren

journal.cli.think.validation.facet.requires.activity = { $facet_flag } erfordert { $activity_flag }

journal.cli.think.validation.flush.incompatible = { $flush_flag } ist nicht vereinbar mit { $segments_flag } und { $refresh_flag }

journal.cli.think.validation.flush.requires.segment = { $flush_flag } erfordert { $segment_flag }

journal.cli.think.validation.multi.worker.unlimited.jobs = { $jobs_flag } 0 ist nicht vereinbar mit { $segments_flag } mit mehreren Workern; setze { $jobs_flag } auf eine positive Grenze oder { $segment_workers_flag } 1

journal.cli.think.validation.no.activity.prompts.with.activity = { $no_activity_prompts_flag } lässt sich nicht mit { $activity_flag } kombinieren

journal.cli.think.validation.reactivate.incompatible.refresh = { $reactivate_flag } ist nicht vereinbar mit { $refresh_flag }

journal.cli.think.validation.reactivate.requires.activity = { $reactivate_flag } erfordert { $activity_flag }

journal.cli.think.validation.repair.segment = { $repair_segment_flag } erfordert { $day_flag } und { $segment_flag } und lässt sich nicht mit einem anderen Denkmodus kombinieren

journal.cli.think.validation.retry.unit = { $retry_unit_flag } erfordert { $day_flag } und { $retry_cause_flag } ohne einen anderen Denkmodus

journal.cli.think.validation.segment.workers.range = { $segment_workers_flag } muss zwischen 1 und 32 liegen

journal.cli.think.validation.segments.incompatible = { $segments_flag } ist nicht vereinbar mit { $segment_flag } und { $facet_flag }

journal.cli.think.validation.settle.incompatible = { $settle_flag } ist nicht vereinbar mit { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag }, { $refresh_flag }, { $weekly_flag } und { $cadence_flag }

journal.cli.think.validation.weekly.incompatible = { $weekly_flag } ist nicht vereinbar mit { $segment_flag }, { $segments_flag }, { $activity_flag } und { $flush_flag }

journal.cli.think.validation.work.intent = { $work_intent_flag } erfordert eine tägliche Anfrage von Grund auf

journal.cli.think.wait.cortex = cortex-Warten fehlgeschlagen: { $error }

journal.cli.think.wait.failed = Warten fehlgeschlagen: { $error }

journal.cli.transcribe.audio.directory = Audiodatei muss in einem Segmentverzeichnis ({ $pattern }) liegen, aber übergeordnetes Verzeichnis ist: { $parent }

journal.cli.transcribe.audio.format = Nicht unterstütztes Audioformat: { $extension }. Unterstützte Formate: { $formats }

journal.cli.transcribe.audio.missing = Audiodatei nicht gefunden.{ $line_break }  Absolut versucht:         { $absolute }{ $line_break }  Journal-relativ versucht: { $relative }

journal.cli.transcribe.backend.choice = Argument { $backend_flag }: ungültige Auswahl: '{ $backend }' (wähle aus { $choices })

journal.cli.transcribe.backend.duplicate = Argument { $backend_flag }: darf nur einmal angegeben werden

journal.cli.transcribe.batch.day = { $count ->
    [one] { $verb }: { $count } Tag konnte nicht gelesen werden
   *[other] { $verb }: { $count } Tage konnten nicht gelesen werden
    }

journal.cli.transcribe.batch.day.record = { $verb }: { $count } Tage konnten nicht gelesen werden

journal.cli.transcribe.batch.failed = Transkription: { $failed } fehlgeschlagen

journal.cli.transcribe.claim.input.cancelled = Überprüfung der Verarbeitungseingabe abgebrochen

journal.cli.transcribe.claim.input.changed.commit = Verarbeitungseingabe hat sich vor der Veröffentlichung geändert

journal.cli.transcribe.claim.input.changed.open = Verarbeitungseingabe hat sich beim Öffnen geändert

journal.cli.transcribe.claim.input.changed.read = Verarbeitungseingabe hat sich beim Lesen geändert

journal.cli.transcribe.claim.input.no.attempt = Eingabe hat keinen Ausgabeversuch

journal.cli.transcribe.claim.input.not.file = Verarbeitungseingabe ist keine einfache Datei

journal.cli.transcribe.claim.lock.entry.unsafe = dauerhafter Sperreintrag ist unsicher ({ $kind }): { $path }

journal.cli.transcribe.claim.lock.invalid = ungültiger dauerhafter Sperreintrag { $name }

journal.cli.transcribe.claim.lock.missing = übergeordnetes Verzeichnis der dauerhaften Sperre fehlt: { $path }

journal.cli.transcribe.claim.lock.mode = dauerhafter Sperreintrag hat Modus { $mode }, erwartet 600: { $path }

journal.cli.transcribe.claim.lock.namespace = dauerhafter Sperreintrag hat sich beim Erwerb geändert: { $path }

journal.cli.transcribe.claim.lock.parent.changed = übergeordnetes Verzeichnis der dauerhaften Sperre hat sich beim Erwerb geändert: { $path }

journal.cli.transcribe.claim.lock.parent.unsafe = übergeordnetes Verzeichnis der dauerhaften Sperre ist unsicher ({ $kind }): { $path }

journal.cli.transcribe.claim.parent.missing = übergeordnetes Verzeichnis fehlt

journal.cli.transcribe.claim.root.absent = Journal-Wurzel existiert nicht

journal.cli.transcribe.claim.root.absolute = Journal-Wurzel muss absolut sein

journal.cli.transcribe.claim.root.ancestor = kanonische Journal-Wurzel hat einen Nicht-UTF-8-Vorfahren

journal.cli.transcribe.claim.root.canonical.absolute = kanonische Journal-Wurzel ist nicht absolut

journal.cli.transcribe.claim.root.changed = Journal-Quelle hat sich beim Erwerb geändert

journal.cli.transcribe.claim.root.component = kanonische Journal-Wurzel hat keine letzte Komponente

journal.cli.transcribe.claim.root.directory = Journal-Wurzel ist kein Verzeichnis

journal.cli.transcribe.claim.root.invalid = ungültige Journal-Wurzel { $path }: { $detail }

journal.cli.transcribe.claim.root.io = { $operation }: { $reason }

journal.cli.transcribe.claim.root.unsupported = nicht unterstützte Journal-Wurzel { $path }: { $detail }

journal.cli.transcribe.claim.segment.not.covered = Segment ist nicht von diesen Ausgabeversuchen abgedeckt

journal.cli.transcribe.claim.target.name = Name des Sperrziels fehlt

journal.cli.transcribe.config.backend = { $key } in der Journal-Konfiguration ist ungültig; wähle eines von: { $backends }

journal.cli.transcribe.error.asset.absent = nicht leeres Modell-Asset { $asset } konnte nicht gefunden werden; gesucht in { $paths }

journal.cli.transcribe.error.asset.digest = Modell-Asset { $asset } unter { $path } hat { $code_sha256 } { $actual }, erwartet { $expected }

journal.cli.transcribe.error.asset.executable = aktuelle Programmdatei konnte nicht ermittelt werden: { $error }

journal.cli.transcribe.error.asset.override = { $code_solstone_transcribe_model_assets_dir }={ $directory } stellt das nicht leere Modell-Asset { $asset } nicht bereit

journal.cli.transcribe.error.audio.budget = Budget dekodierter Abtastwerte { $limit } für { $path } überschritten

journal.cli.transcribe.error.audio.corrupt = beschädigte Audioeingabe { $path }: { $detail }

journal.cli.transcribe.error.audio.empty = Audioeingabe ist leer: { $path }

journal.cli.transcribe.error.audio.ffmpeg = { $code_ffmpeg }-Fehler für { $path }: { $detail }

journal.cli.transcribe.error.audio.flush = { $iterations ->
    [one] { $flushed ->
        [one] Resampler-Leerung ist für { $path } nach { $iterations } Durchlauf nicht konvergiert ({ $flushed } Abtastwert geleert; { $remaining } noch verzögert)
        *[other] Resampler-Leerung ist für { $path } nach { $iterations } Durchlauf nicht konvergiert ({ $flushed } Abtastwerte geleert; { $remaining } noch verzögert)
    }
    *[other] { $flushed ->
        [one] Resampler-Leerung ist für { $path } nach { $iterations } Durchläufen nicht konvergiert ({ $flushed } Abtastwert geleert; { $remaining } noch verzögert)
        *[other] Resampler-Leerung ist für { $path } nach { $iterations } Durchläufen nicht konvergiert ({ $flushed } Abtastwerte geleert; { $remaining } noch verzögert)
    }
    }

journal.cli.transcribe.error.audio.io = { $code_i_o }-Fehler für { $path }: { $error }

journal.cli.transcribe.error.audio.sample.rate = ungültige { $code_wav }-Abtastrate: { $rate }

journal.cli.transcribe.error.audio.samples = keine Audio-Abtastwerte aus { $path } dekodiert

journal.cli.transcribe.error.audio.sidecar.create = { $code_f32le }-sidecar { $path } kann nicht erstellt werden: { $error }

journal.cli.transcribe.error.audio.sidecar.sync = { $code_f32le }-sidecar { $path } kann nicht abgeglichen werden: { $error }

journal.cli.transcribe.error.audio.sidecar.write = { $code_f32le }-sidecar { $path } kann nicht geschrieben werden: { $error }

journal.cli.transcribe.error.audio.stream = kein Audio-Datenstrom in { $path }

journal.cli.transcribe.error.audio.wav.size = { $samples ->
    [one] WAV-Daten sind für ein { $riff_u32 }-Feld zu groß: { $samples } Abtastwert
   *[other] WAV-Daten sind für ein { $riff_u32 }-Feld zu groß: { $samples } Abtastwerte
    }

journal.cli.transcribe.error.backend.memory = kein nutzbares { $code_stt }-backend (verfügbarer Arbeitsspeicher: { $available }, lokale Untergrenze: { $floor })

journal.cli.transcribe.error.backend.platform = { $code_stt }-backend ist auf dieser Host-Plattform nicht verfügbar: { $backend }

journal.cli.transcribe.error.cancelled = eingebettete Transkription vor der Veröffentlichung abgebrochen

journal.cli.transcribe.error.claim = Eigentum an der Transkription für { $path } konnte nicht erworben werden: { $detail }

journal.cli.transcribe.error.confidential.confidential.audio.handling.is.disabled = vertrauliche Audio-Handhabung ist deaktiviert

journal.cli.transcribe.error.confidential.confidential.lane.blocks.stt.backend.backend.raw.audio = vertrauliche Lane blockiert { $code_stt }-backend { $backend }; Roh-Audio muss lokal bleiben

journal.cli.transcribe.error.confidential.failure = vertrauliche Transkription { $reason }: { $detail }

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.status = { $code_stt } (gehostet) hat { $code_http } { $status } zurückgegeben

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.unauthorized = { $code_stt } (gehostet) hat { $code_http } 401 zurückgegeben

journal.cli.transcribe.error.confidential.the.confidential.admission.probe.failed = die vertrauliche Zulassungssonde ist fehlgeschlagen

journal.cli.transcribe.error.confidential.the.confidential.attestation.channel.is.not.ready = der vertrauliche Attestierungskanal ist nicht bereit

journal.cli.transcribe.error.confidential.the.confidential.endpoint.has.no.credential = der vertrauliche endpoint hat keine Zugangsdaten

journal.cli.transcribe.error.confidential.the.confidential.lane.has.no.confidential.byo.endpoint = die vertrauliche Lane hat keinen vertraulichen { $code_byo }-endpoint

journal.cli.transcribe.error.confidential.the.confidential.lane.is.no.longer.active = die vertrauliche Lane ist nicht mehr aktiv

journal.cli.transcribe.error.confidential.word.a.word.entry.was.blank = ein Worteintrag war unausgefüllt

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.a.required.key = einem Worteintrag fehlte ein erforderlicher Schlüssel

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.end = einem Worteintrag fehlte `end`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.start = einem Worteintrag fehlte `start`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.word = einem Worteintrag fehlte `word`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.not.an.object = ein Worteintrag war kein Objekt

journal.cli.transcribe.error.confidential.word.contract = Antwort von { $code_stt } (gehostet) hat den ausführlichen { $code_json }-Vertrag verletzt: { $detail }

journal.cli.transcribe.error.confidential.word.response.had.no.words.array = Antwort hatte kein Wörter-Array

journal.cli.transcribe.error.confidential.word.response.had.text.but.no.word.timings = Antwort hatte Text, aber keine Wort-Zeitangaben

journal.cli.transcribe.error.confidential.word.response.was.not.an.object = Antwort war kein Objekt

journal.cli.transcribe.error.confidential.word.response.was.not.json = Antwort war nicht { $code_json }

journal.cli.transcribe.error.confidential.word.word.key.was.not.a.finite.number.found = Wort `{ $key }` war keine endliche Zahl (gefunden { $found })

journal.cli.transcribe.error.coreml.failure = { $code_parakeet } { $code_coreml } { $reason }: { $detail }

journal.cli.transcribe.error.coreml.helper.json.was.not.an.object = Helfer-{ $code_json } war kein Objekt

journal.cli.transcribe.error.coreml.helper.response.has.invalid.key = Helfer-Antwort: { $key } ist ungültig

journal.cli.transcribe.error.coreml.helper.response.missing.key = Helfer-Antwort: { $key } fehlt

journal.cli.transcribe.error.coreml.helper.response.missing.token.timings = Helfer-Antwort: { $code_token_timings } fehlt

journal.cli.transcribe.error.coreml.helper.returned.transcript.text.without.token.timings = Helfer hat Transkripttext ohne Token-Zeitangaben zurückgegeben

journal.cli.transcribe.error.coreml.helper.token.timings.collapsed.to.no.words = Token-Zeitangaben des Helfers sind zu keinen Wörtern zusammengefallen

journal.cli.transcribe.error.coreml.helper.version.json.was.not.an.object = Helfer-Version { $code_json } war kein Objekt

journal.cli.transcribe.error.coreml.helper.version.response.missing.field = Antwort der Helfer-Version: { $field } fehlt

journal.cli.transcribe.error.coreml.missing.stderr.pipe = { $code_stderr }-Pipe fehlt

journal.cli.transcribe.error.coreml.missing.stdout.pipe = { $code_stdout }-Pipe fehlt

journal.cli.transcribe.error.coreml.name.reader.panicked = { $name }-Leser hat Panic ausgelöst

journal.cli.transcribe.error.coreml.parakeet.coreml.model.is.not.installed.yet.path = { $code_parakeet }-{ $code_coreml }-Modell ist noch nicht installiert: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.exited.with.status = { $code_parakeet }-Helfer wurde mit { $status } beendet

journal.cli.transcribe.error.coreml.parakeet.helper.is.missing.path = { $code_parakeet }-Helfer fehlt: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.a.file.path = { $code_parakeet }-Helfer ist keine Datei: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.executable.path = { $code_parakeet }-Helfer ist nicht ausführbar: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out = { $code_parakeet }-Helfer hat die Zeit überschritten

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out.after.seconds.s = { $code_parakeet }-Helfer hat nach { $seconds } s die Zeit überschritten

journal.cli.transcribe.error.coreml.publication.busy = { $code_parakeet }-{ $code_coreml }-Veröffentlichungssperre belegt

journal.cli.transcribe.error.coreml.token.timing.must.be.an.object = Token-Zeitangabe muss ein Objekt sein

journal.cli.transcribe.error.cpp.device.must.be.one.of.auto.cpu = Gerät muss eines von diesen sein: { $code_auto_cpu }

journal.cli.transcribe.error.cpp.failure = { $code_parakeet_cpp } { $reason }: { $detail }

journal.cli.transcribe.error.cpp.http.status = { $code_http } { $status }

journal.cli.transcribe.error.cpp.parakeet.cpp.is.only.supported.on.linux.and = { $code_parakeet_cpp } wird nur auf Linux und Windows unterstützt

journal.cli.transcribe.error.cpp.parakeet.server.credentials.are.unavailable.in.this.process = { $code_parakeet }-Server-Zugangsdaten sind in diesem Prozess nicht verfügbar.

journal.cli.transcribe.error.cpp.parakeet.server.is.not.ready.yet = { $code_parakeet }-Server ist noch nicht bereit.

journal.cli.transcribe.error.cpp.response.has.text.but.no.word.timings = Antwort hat Text, aber keine Wort-Zeitangaben

journal.cli.transcribe.error.cpp.response.json.was.invalid.error = Antwort-{ $code_json } war ungültig: { $error }

journal.cli.transcribe.error.cpp.response.json.was.not.an.object = Antwort-{ $code_json } war kein Objekt

journal.cli.transcribe.error.cpp.response.missing.top.level.words = Antwort: { $code_words } auf oberster Ebene fehlt

journal.cli.transcribe.error.cpp.tls.is.required = { $code_tls } ist erforderlich

journal.cli.transcribe.error.cpp.transcription.request.timed.out = Transkriptionsanfrage hat die Zeit überschritten

journal.cli.transcribe.error.cpp.word.timing.contains.invalid.numeric.value = Wort-Zeitangabe enthält einen ungültigen Zahlenwert

journal.cli.transcribe.error.cpp.word.timing.item.must.be.an.object = Element der Wort-Zeitangabe muss ein Objekt sein

journal.cli.transcribe.error.cpp.word.timing.missing.key.key = Wort-Zeitangabe: Schlüssel { $key } fehlt

journal.cli.transcribe.error.cpp.word.timing.text.was.blank = Text der Wort-Zeitangabe war unausgefüllt

journal.cli.transcribe.error.input.inspect = Eingabe { $path } konnte nicht untersucht werden: { $detail }

journal.cli.transcribe.error.orphan = verwaiste { $code_npz }-Datei { $path } konnte nicht entfernt werden: { $detail }

journal.cli.transcribe.error.payload.parent = Transkriptziel hat kein übergeordnetes Verzeichnis

journal.cli.transcribe.error.process.admission = gehostete Startzulassung ist fehlgeschlagen: { $reason }

journal.cli.transcribe.error.process.capability = Host-Fähigkeit nicht verfügbar: { $needed }

journal.cli.transcribe.error.process.command = ein leerer Befehl kann nicht erzeugt werden

journal.cli.transcribe.error.process.confirm = Bestätigung nach dem Erzeugen ist für pid { $pid } fehlgeschlagen: { $error }

journal.cli.transcribe.error.process.explicit = diese Autorität ist nicht ausdrücklich ohne Eigentümer

journal.cli.transcribe.error.process.identity = geburtsgebundene Identität für erzeugte pid { $pid } konnte nicht erfasst werden

journal.cli.transcribe.error.process.log = betriebliches Protokoll konnte nicht vorbereitet werden: { $error }

journal.cli.transcribe.error.process.output = Ausgabe des Kindprozesses ist nicht verfügbar

journal.cli.transcribe.error.process.spawn = Kindprozess konnte nicht erzeugt werden: { $error }

journal.cli.transcribe.error.process.terminate = Kindprozess konnte nicht beendet werden: { $error }

journal.cli.transcribe.error.process.unowned.reason = { $code_explicitlyunowned }-Grund darf nicht leer sein

journal.cli.transcribe.error.publish.output = { $code_output_jsonl_path } ist erforderlich

journal.cli.transcribe.error.publish.parent = Ausgabe hat kein übergeordnetes Verzeichnis

journal.cli.transcribe.error.publish.raw = { $code_header_raw } ist erforderlich

journal.cli.transcribe.error.publish.sibling = Roh der Verarbeitung muss ein Geschwister-Dateiname sein

journal.cli.transcribe.error.speaker.analysis = Sprecheranalyse ist fehlgeschlagen: { $stage }/{ $reason }

journal.cli.transcribe.error.statement.field = vollständige Transkript-Äußerung: kein gültiger Wert für { $field }

journal.cli.transcribe.error.terminal.payload = Terminal-Nutzdaten konnten nicht vorbereitet werden: { $detail }

journal.cli.transcribe.error.terminal.payload.path = Terminal-Nutzdaten { $path } konnten nicht vorbereitet oder entfernt werden: { $detail }

journal.cli.transcribe.error.terminal.request = Terminal-Schreibanfrage konnte nicht serialisiert werden: { $detail }

journal.cli.transcribe.error.transcript.payload = Transkript-Nutzdaten konnten nicht vorbereitet werden: { $detail }

journal.cli.transcribe.error.transcript.payload.path = Transkript-Nutzdaten { $path } konnten nicht vorbereitet oder entfernt werden: { $detail }

journal.cli.transcribe.error.transcript.request = Transkript-Schreibanfrage konnte nicht serialisiert werden: { $detail }

journal.cli.transcribe.error.vad.contract.exit = { $code_vad }-Helfer-Vertragsfehler (Exit { $code }): { $detail }

journal.cli.transcribe.error.vad.contract.exit.stderr = { $code_vad }-Helfer-Vertragsfehler (Exit { $code }): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.contract.signal = { $code_vad }-Helfer-Vertragsfehler (kein Exit-Code): { $detail }

journal.cli.transcribe.error.vad.contract.signal.stderr = { $code_vad }-Helfer-Vertragsfehler (kein Exit-Code): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.could.not.create.vad.input.directory.error = { $code_vad }-Eingabeverzeichnis konnte nicht erstellt werden: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.error = { $code_vad }-Eingabeverzeichnis konnte nicht entfernt werden: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.input.remains.in = { $code_vad }-Eingabeverzeichnis konnte nicht entfernt werden: Eingabe wird noch verwendet

journal.cli.transcribe.error.vad.could.not.send.vad.request.error = die { $code_vad }-Anfrage konnte nicht gesendet werden: { $error }

journal.cli.transcribe.error.vad.could.not.serialize.vad.request.error = die { $code_vad }-Anfrage konnte nicht serialisiert werden: { $error }

journal.cli.transcribe.error.vad.error.empty = { $code_vad }-Fehlerantwort war leer

journal.cli.transcribe.error.vad.error.many = { $code_vad }-Fehlerantwort enthielt mehr als eine Zeile

journal.cli.transcribe.error.vad.error.utf8 = { $code_vad }-Fehlerantwort war nicht { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.helper = { $code_vad }-Helfer { $reason } (Exit { $code }): { $detail }

journal.cli.transcribe.error.vad.launch.failed = { $code_vad }-Helfer { $path } konnte nicht gestartet werden: { $detail }

journal.cli.transcribe.error.vad.response.empty = { $code_vad }-Antwort war leer

journal.cli.transcribe.error.vad.response.many = { $code_vad }-Antwort enthielt mehr als eine Zeile

journal.cli.transcribe.error.vad.response.utf8 = { $code_vad }-Antwort war nicht { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.vad.error.response.lacks.schema.reason.or.detail = { $code_vad }-Fehlerantwort ohne Schema, Grund oder Detail

journal.cli.transcribe.error.vad.vad.error.response.must.be.an.object = { $code_vad }-Fehlerantwort muss ein Objekt sein

journal.cli.transcribe.error.vad.vad.error.response.violates.its.schema = { $code_vad }-Fehlerantwort verletzt ihr Schema

journal.cli.transcribe.error.vad.vad.error.response.was.not.json.error = { $code_vad }-Fehlerantwort war nicht { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.helper.stdin.was.unavailable = { $code_vad }-Helfer: { $code_stdin } war nicht verfügbar

journal.cli.transcribe.error.vad.vad.helper.terminated.without.an.exit.code = { $code_vad }-Helfer wurde ohne Exit-Code beendet

journal.cli.transcribe.error.vad.vad.response.has.an.unknown.schema = { $code_vad }-Antwort hat ein unbekanntes Schema

journal.cli.transcribe.error.vad.vad.response.has.no.boolean.has.speech = { $code_vad }-Antwort hat keinen booleschen Wert für { $code_has_speech }

journal.cli.transcribe.error.vad.vad.response.has.no.non.empty.field = { $code_vad }-Antwort hat keinen nicht leeren Wert für { $field }

journal.cli.transcribe.error.vad.vad.response.has.no.non.negative.finite.field = { $code_vad }-Antwort hat keinen nicht-negativen endlichen Wert für { $field }

journal.cli.transcribe.error.vad.vad.response.has.no.speech.array = { $code_vad }-Antwort hat kein Gesprochenes-Array

journal.cli.transcribe.error.vad.vad.response.must.be.an.object = { $code_vad }-Antwort muss ein Objekt sein

journal.cli.transcribe.error.vad.vad.response.was.not.json.error = { $code_vad }-Antwort war nicht { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.speech.entry.ends.before.it.starts = { $code_vad }-Eintrag für Gesprochenes endet, bevor er beginnt

journal.cli.transcribe.error.vad.vad.speech.entry.has.no.valid.field.sample.index = { $code_vad }-Eintrag für Gesprochenes hat keinen gültigen { $field }-Abtastwert-Index

journal.cli.transcribe.error.vad.vad.speech.entry.must.be.an.object = { $code_vad }-Eintrag für Gesprochenes muss ein Objekt sein

journal.cli.transcribe.error.vad.wait.failed = auf den { $code_vad }-Helfer konnte nicht gewartet werden: { $detail }

journal.cli.transcribe.error.vad.write = { $code_vad }-Eingabe konnte nicht geschrieben werden: { $detail }

journal.cli.transcribe.error.voice.wait = Transkription nur mit Stimme wartet: { $detail }

journal.cli.transcribe.error.windows.admission.deadline.exceeded = Zulassung des begrenzten Helfers wurde nicht vor ihrer Frist abgeschlossen

journal.cli.transcribe.error.windows.blocked = { $detail }; vorheriges Aufräumen des Helfers blockiert die Zulassung: { $blockers }

journal.cli.transcribe.error.windows.cancelled = begrenzter Helfer wurde abgebrochen

journal.cli.transcribe.error.windows.cleanup = { $detail }; Aufräumen war bei der Rückkehr unvollständig: { $cleanup }

journal.cli.transcribe.error.windows.cleanup.blocked = { $detail }; Aufräumen war bei der Rückkehr unvollständig: { $cleanup }; vorheriges Aufräumen des Helfers blockiert die Zulassung: { $blockers }

journal.cli.transcribe.error.windows.cleanup.contended = Zulassung zum Aufräumen des begrenzten Helfers ist umkämpft

journal.cli.transcribe.error.windows.cleanup.pending = früheres Aufräumen des begrenzten Helfers blockiert die Zulassung

journal.cli.transcribe.error.windows.current.directory.not.directory = aktuelles Verzeichnis des begrenzten Helfers ist kein Verzeichnis

journal.cli.transcribe.error.windows.current.directory.outside.package = aktuelles Verzeichnis des begrenzten Helfers liegt außerhalb seiner Paketwurzel

journal.cli.transcribe.error.windows.current.directory.unavailable = aktuelles Verzeichnis des begrenzten Helfers konnte nicht kanonisiert werden

journal.cli.transcribe.error.windows.deadline.exceeded = begrenzter Helfer wurde nicht vor seiner Frist abgeschlossen

journal.cli.transcribe.error.windows.executable.not.file = Programmdatei des begrenzten Helfers ist keine reguläre Datei

journal.cli.transcribe.error.windows.executable.outside.package = Programmdatei des begrenzten Helfers liegt außerhalb seiner Paketwurzel

journal.cli.transcribe.error.windows.executable.unavailable = Programmdatei des begrenzten Helfers konnte nicht kanonisiert werden

journal.cli.transcribe.error.windows.input.limit.exceeded = Eingabe des begrenzten Helfers überschreitet das angegebene Byte-Limit

journal.cli.transcribe.error.windows.input.write.failed = Eingabeschreiber des begrenzten Helfers ist fehlgeschlagen

journal.cli.transcribe.error.windows.io.worker.start.failed = { $stream }-{ $code_i_o }-Worker des begrenzten Helfers konnte nicht gestartet werden

journal.cli.transcribe.error.windows.job.not.quiescent = Abschluss des begrenzten Helfers hat keine Job-Ruhe hergestellt

journal.cli.transcribe.error.windows.launch.failed = begrenzter Helfer ist fehlgeschlagen, bevor eine Identität des eigenen Kindprozesses verfügbar war

journal.cli.transcribe.error.windows.launch.finalization.failed = nativer Startabschluss ist fehlgeschlagen: { $detail }

journal.cli.transcribe.error.windows.missing.system.root = Umgebung des begrenzten Helfers muss { $code_systemroot } enthalten, und das darf nicht leer sein

journal.cli.transcribe.error.windows.output.limit.exceeded = Ausgabe { $stream } des begrenzten Helfers hat das angegebene Bytelimit überschritten

journal.cli.transcribe.error.windows.output.read.failed = Ausgabeleser { $stream } des begrenzten Helfers ist fehlgeschlagen

journal.cli.transcribe.error.windows.package.root.unavailable = Paketwurzel des begrenzten Helfers konnte nicht kanonisiert werden

journal.cli.transcribe.error.windows.path.environment.refused = Umgebung des begrenzten Helfers darf { $code_path } nicht enthalten

journal.cli.transcribe.error.windows.path.not.representable = Pfad des begrenzten Helfers kann von der verwalteten Windows-Befehlsgrenze nicht dargestellt werden

journal.cli.transcribe.error.windows.process.observation.failed = Prozessbeobachtung des begrenzten Helfers ist fehlgeschlagen

journal.cli.transcribe.error.windows.zero.limit = Bytelimit { $stream } des begrenzten Helfers muss ungleich null sein

journal.cli.transcribe.error.windows.zero.timeout = Zeitüberschreitung des begrenzten Helfers muss ungleich null sein

journal.cli.transcribe.error.writer.base.time.us.of.day.is.outside.a.day = { $code_base_time_us_of_day } liegt außerhalb eines Tages

journal.cli.transcribe.error.writer.byte.count.does.not.match.shape = { $code_byte_count } stimmt nicht mit der Form überein

journal.cli.transcribe.error.writer.byte.count.is.too.large = { $code_byte_count } ist zu groß

journal.cli.transcribe.error.writer.cancelled = Veröffentlichung des Transkripts wurde vor der Zulassung abgebrochen

journal.cli.transcribe.error.writer.destination = Ziel ist bereits vorhanden: { $path }

journal.cli.transcribe.error.writer.dtype.must.be.float32.le = { $code_dtype } muss { $code_float32_le } sein

journal.cli.transcribe.error.writer.duration.and.noisy.must.be.provided.together = duration und noisy müssen zusammen angegeben werden

journal.cli.transcribe.error.writer.durations.s.count.does.not.match.shape = { $code_durations_s }-Anzahl stimmt nicht mit der Form überein

journal.cli.transcribe.error.writer.durations.s.must.be.finite.f32.values = { $code_durations_s } muss endliche { $code_f32 }-Werte sein

journal.cli.transcribe.error.writer.encoder.must.be.nonempty = Encoder darf nicht leer sein

journal.cli.transcribe.error.writer.field.must.be.a.boolean = { $field } muss ein boolescher Wert sein

journal.cli.transcribe.error.writer.field.must.be.a.finite.number = { $field } muss eine endliche Zahl sein

journal.cli.transcribe.error.writer.field.must.be.a.string = { $field } muss eine Zeichenkette sein

journal.cli.transcribe.error.writer.field.must.be.an.array = { $field } muss ein Array sein

journal.cli.transcribe.error.writer.field.must.be.an.integer = { $field } muss eine Ganzzahl sein

journal.cli.transcribe.error.writer.field.must.be.an.unsigned.integer = { $field } muss eine vorzeichenlose Ganzzahl sein

journal.cli.transcribe.error.writer.header.numbers.must.be.finite = Header-Zahlen müssen endlich sein

journal.cli.transcribe.error.writer.id.range = ID muss in { $code_1_i32_max } liegen

journal.cli.transcribe.error.writer.invalid.f32.payload.chunk = ungültiger { $code_f32 }-Nutzdatenblock

journal.cli.transcribe.error.writer.jsonl.path.and.npz.path.must.be.sibling.files = { $code_jsonl_path } und { $code_npz_path } müssen Geschwisterdateien mit gemeinsamem Stamm sein

journal.cli.transcribe.error.writer.missing.required.field.key = erforderliches Feld { $key } fehlt

journal.cli.transcribe.error.writer.noisy.s.is.required.when.noisy.rms.is.present = { $code_noisy_s } ist erforderlich, wenn { $code_noisy_rms } vorhanden ist

journal.cli.transcribe.error.writer.npz.cancelled = abgebrochen

journal.cli.transcribe.error.writer.npz.count = { $code_npz } hat eine unerwartete Anzahl an Einträgen

journal.cli.transcribe.error.writer.npz.members = { $code_npz }-Einträge stimmen nicht mit dem sidecar-Vertrag des Transkripts überein

journal.cli.transcribe.error.writer.npz.missing = { $code_npz }-Eintrag { $name } fehlt

journal.cli.transcribe.error.writer.npz.parent = npz-Ziel hat kein übergeordnetes Verzeichnis

journal.cli.transcribe.error.writer.npz.readback = { $code_npz }-Eintrag { $name } hat den Roundtrip nicht bestanden

journal.cli.transcribe.error.writer.output.path.has.no.parent = Ausgabepfad hat kein übergeordnetes Verzeichnis

journal.cli.transcribe.error.writer.output.paths.must.be.nonempty = Ausgabepfade dürfen nicht leer sein

journal.cli.transcribe.error.writer.payload.finite = Einbettungs-Nutzdaten sind nicht endlich bei Zeile { $row }, Spalte { $column }

journal.cli.transcribe.error.writer.payload.format.must.be.raw.f32le.row.major.v1 = { $code_payload_format } muss { $code_raw_f32le_row_major_v1 } sein

journal.cli.transcribe.error.writer.payload.size.does.not.match.byte.count = Nutzdatengröße stimmt nicht mit { $code_byte_count } überein

journal.cli.transcribe.error.writer.prior.jsonl.snapshot.is.missing = vorheriges jsonl-Abbild fehlt

journal.cli.transcribe.error.writer.prior.npz.snapshot.is.missing = vorheriges npz-Abbild fehlt

journal.cli.transcribe.error.writer.prior.restore.refused = vorherige Wiederherstellung abgelehnt

journal.cli.transcribe.error.writer.private.stage.is.missing = private Stufe fehlt

journal.cli.transcribe.error.writer.private.transcript.copy.did.not.match.the.staged.file = private Transkriptkopie stimmt nicht mit der Datei in der Stufe überein

journal.cli.transcribe.error.writer.private.transcript.jsonl.is.missing = private Transkript-jsonl fehlt

journal.cli.transcribe.error.writer.rounded.header.number.is.not.json.representable = gerundete Header-Zahl ist nicht { $code_json }-darstellbar

journal.cli.transcribe.error.writer.schema = unbekanntes Schema: { $schema }

journal.cli.transcribe.error.writer.segment.meta.must.be.an.object = { $code_segment_meta } muss ein Objekt sein

journal.cli.transcribe.error.writer.shape.byte.count.overflows = Byteanzahl der Form läuft über

journal.cli.transcribe.error.writer.shape.must.be.row.count.width = Form muss { $code_row_count_256 } sein

journal.cli.transcribe.error.writer.shape.row.count.is.too.large = Zeilenanzahl der Form ist zu groß

journal.cli.transcribe.error.writer.speaker.evidence.multi.fraction.is.required.with.speaker.evidence = { $code_speaker_evidence_multi_fraction } ist erforderlich mit { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.evidence.version.is.required.with.speaker.evidence = { $code_speaker_evidence_version } ist erforderlich mit { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.must.be.a.string.integer.null.or.absent = Sprecher muss eine Zeichenkette, eine Ganzzahl, null oder nicht vorhanden sein

journal.cli.transcribe.error.writer.speech.loud.windows.is.required.when.loud.windows.is = { $code_speech_loud_windows } ist erforderlich, wenn { $code_loud_windows } positiv ist

journal.cli.transcribe.error.writer.stage.path.already.exists = Stufenpfad ist bereits vorhanden

journal.cli.transcribe.error.writer.stage.path.is.a.symlink = Stufenpfad ist eine symbolische Verknüpfung

journal.cli.transcribe.error.writer.stage.path.is.not.a.private.directory = Stufenpfad ist kein privates Verzeichnis

journal.cli.transcribe.error.writer.stale.npz.unlink.refused = veraltetes npz-unlink abgelehnt

journal.cli.transcribe.error.writer.statement = Äußerung { $index }: { $detail }

journal.cli.transcribe.error.writer.statement.duplicate = Äußerung { $index } dupliziert ID { $id }

journal.cli.transcribe.error.writer.statement.id.is.not.a.transcript.statement.id = Äußerungs-ID ist keine Transkript-Äußerungs-ID

journal.cli.transcribe.error.writer.statement.id.is.outside.i32 = Äußerungs-ID liegt außerhalb von { $code_i32 }

journal.cli.transcribe.error.writer.statement.id.missing = bei Äußerung { $index } fehlt die ID

journal.cli.transcribe.error.writer.statement.ids.count.does.not.match.shape = { $code_statement_ids }-Anzahl stimmt nicht mit der Form überein

journal.cli.transcribe.error.writer.statement.ids.must.be.unique = Äußerungs-IDs müssen eindeutig sein

journal.cli.transcribe.error.writer.statement.text.ended.before.its.trimmed.length = Äußerungstext endete vor seiner gekürzten Länge

journal.cli.transcribe.error.writer.statement.text.is.not.valid.utf8 = Äußerungstext ist kein gültiges { $code_utf_8 }

journal.cli.transcribe.error.writer.text.must.be.a.nonempty.string = Text muss eine nicht leere Zeichenkette sein

journal.cli.transcribe.error.writer.turn.audio.duration.s.must.be.non.negative = { $code_audio_duration_s } darf nicht negativ sein

journal.cli.transcribe.error.writer.turn.boundary.suspect.must.be.true.if.present = { $code_turn_boundary_suspect } muss true sein, wenn vorhanden

journal.cli.transcribe.error.writer.turn.expected.version.expected.got.version = erwartete Version { $expected }, erhalten { $version }

journal.cli.transcribe.error.writer.turn.field.is.missing = { $field } fehlt

journal.cli.transcribe.error.writer.turn.field.must.be.finite = { $field } muss endlich sein

journal.cli.transcribe.error.writer.turn.invalid = ungültig { $code_speaker_turn_evidence }: { $detail }

journal.cli.transcribe.error.writer.turn.range.index.end.s.end.s.exceeds.audio = Bereich { $index } { $code_end_s } { $end_s } überschreitet { $code_audio_duration_s } { $audio_duration_s }

journal.cli.transcribe.error.writer.turn.range.index.has.unexpected.keys = Bereich { $index } hat unerwartete Schlüssel

journal.cli.transcribe.error.writer.turn.range.index.is.an.exact.duplicate = Bereich { $index } ist ein exaktes Duplikat

journal.cli.transcribe.error.writer.turn.range.index.is.inverted.start.s.start.s = Bereich { $index } ist invertiert: { $code_start_s } { $start_s } > { $code_end_s } { $end_s }

journal.cli.transcribe.error.writer.turn.range.index.must.be.an.object = Bereich { $index } muss ein Objekt sein

journal.cli.transcribe.error.writer.turn.range.index.start.s.must.be.non.negative = Bereich { $index } { $code_start_s } darf nicht negativ sein

journal.cli.transcribe.error.writer.turn.ranges.must.be.an.array = Bereiche müssen ein Array sein

journal.cli.transcribe.error.writer.turn.ranges.must.be.sorted.range.index.is.out = Bereiche müssen sortiert sein: Bereich { $index } ist außerhalb der Reihenfolge

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.has.unexpected.keys = Beleg der Sprecherwechsel hat unerwartete Schlüssel

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.must.be.an.object = Beleg der Sprecherwechsel muss ein Objekt sein

journal.cli.transcribe.error.writer.turn.version.must.be.a.string = Version muss eine Zeichenkette sein

journal.cli.transcribe.error.writer.zip.central.extra.at.extra.at.error = zentrales Extra bei { $extra_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.header.at.central.header.start.error = zentraler Header bei { $central_header_start }: { $error }

journal.cli.transcribe.error.writer.zip.central.name.at.name.at.error = zentraler Name bei { $name_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.record.extends.beyond.archive = zentraler Datensatz reicht über das Archiv hinaus

journal.cli.transcribe.error.writer.zip.central.signature.at.central.header.start = zentrale Signatur bei { $central_header_start }

journal.cli.transcribe.error.writer.zip.cp437.scalar = { $code_cp437 }-Skalar

journal.cli.transcribe.error.writer.zip.flagged.name.is.not.utf8 = markierter Name ist nicht { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.local.header.at.header.start.error = lokaler Header bei { $header_start }: { $error }

journal.cli.transcribe.error.writer.zip.local.signature.at.header.start = lokale Signatur bei { $header_start }

journal.cli.transcribe.error.writer.zip.raw.and.exposed.central.populations.disagree = rohe und offengelegte zentrale Populationen stimmen nicht überein

journal.cli.transcribe.error.writer.zip.raw.central.member.was.lost.from.the.zip = roher zentraler Eintrag ging aus dem ZIP-Index verloren

journal.cli.transcribe.error.writer.zip.repeated.exposed.central.coordinate = wiederholte offengelegte zentrale Koordinate

journal.cli.transcribe.error.writer.zip.unicode.path.extra.crc.mismatch = Unicode-Pfad-Extra: CRC stimmt nicht überein

journal.cli.transcribe.error.writer.zip.unicode.path.extra.is.not.utf8 = Unicode-Pfad-Extra ist nicht { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.unicode.path.extra.too.small = Unicode-Pfad-Extra zu klein

journal.cli.transcribe.error.writer.zip.unproved = zip-Eintrag { $index }: Name nicht nachgewiesen: { $detail }

journal.cli.transcribe.error.writer.zip.unreadable = zip-Archiv nicht lesbar: { $detail }

journal.cli.transcribe.error.writer.zip.zip.member.name.collision.name = Kollision beim Namen des zip-Eintrags: { $name }

journal.cli.transcribe.feed.failure = Einspeisung des Sprecher-Kandidatenpools ist für { $path } fehlgeschlagen: { $detail }

journal.cli.transcribe.feed.parse = Transkript konnte nicht geparst werden: { $detail }

journal.cli.transcribe.feed.pool.lock = Sperre des Kandidatenpools ist fehlgeschlagen: { $detail }

journal.cli.transcribe.feed.pool.read = Lesen des Kandidatenpools ist fehlgeschlagen: { $detail }

journal.cli.transcribe.feed.pool.width = Breite des Schwerpunkts im Kandidatenpool stimmt nicht überein

journal.cli.transcribe.feed.pool.write = Schreiben des Kandidatenpools ist fehlgeschlagen: { $detail }

journal.cli.transcribe.feed.read = Transkript konnte nicht gelesen werden: { $detail }

journal.cli.transcribe.feed.unreadable = abgeschlossenes Transkript oder Einbettungen konnten nicht gelesen werden

journal.cli.transcribe.file.failed = { $file }: Transkription ist fehlgeschlagen ({ $reason }); Details: { $output }

journal.cli.transcribe.help = Aufruf: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $line_break }Audio-Dateien transkribieren, { $stt } steckbar, Sprecheranalyse nativ{ $line_break }{ $line_break }Positionsargumente:{ $line_break }  { $input }            Audio-Dateipfad im Journal-Segmentverzeichnis, z. B.{ $line_break }                        { $sample }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $all_option }{ $gap_17 }Alle unbearbeiteten Audio-Segmente im{ $line_break }                        Journal im Stapel transkribieren{ $line_break }{ $indent_2 }{ $redo_option }{ $gap_16 }Datei erneut verarbeiten, Ausgaben überschreiben{ $line_break }{ $syntax_continuation_16 }{ $line_break }                        { $stt }-backend (überschreibt Konfiguration und ressourcenbewusste{ $line_break }                        Auto-Vorgabe){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Ausführliche Ausgabe aktivieren{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Debug-Protokoll aktivieren

journal.cli.transcribe.installation.capability.mismatch = Generationsfähigkeit stimmt nicht mit der aktuellen Installation überein

journal.cli.transcribe.installation.coded = { $code }: { $detail }

journal.cli.transcribe.installation.fd.platform = { $fd }: nicht unterstützte Plattform

journal.cli.transcribe.installation.grant.kind = unerwartete Art der Generationsgewährung

journal.cli.transcribe.installation.grant.required = authentifizierte Generationsgewährung fehlt oder ist doppelt vorhanden

journal.cli.transcribe.installation.helper.parent = { $helper }: aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.transcribe.installation.id.encoding = nicht-{ $unicode } Generations-ID

journal.cli.transcribe.installation.lock.kind = Generationssperre ist keine gewöhnliche Datei

journal.cli.transcribe.installation.metadata.malformed = fehlerhafte Generationsmetadaten

journal.cli.transcribe.installation.metadata.partial = unvollständige Generationsmetadaten

journal.cli.transcribe.installation.metadata.required = Generationsmetadaten fehlen

journal.cli.transcribe.installation.metadata.unverified = Generationsmetadaten haben keine authentifizierte Startfähigkeit

journal.cli.transcribe.installation.obsolete = veraltetes Kennzeichen des { $windows }-Generationsdeskriptors

journal.cli.transcribe.installation.platform = { $code }: { $platform }/{ $architecture }

journal.cli.transcribe.installation.provenance = Herkunft des Generationsstarts stimmt nicht mit dieser Wurzel überein

journal.cli.transcribe.installation.stamp = { $code }: { $path }: { $error }

journal.cli.transcribe.installation.token.encoding = nicht-{ $unicode } Generations-Token

journal.cli.transcribe.owner.running = dein Journal läuft und transkribiert Aufnahmen selbst. bring die Aufnahme stattdessen mit '{ $import }' herein.

journal.cli.transcribe.owner.stopped = dein Journal läuft nicht. starte es mit '{ $up }' und bring die Aufnahme dann mit '{ $import }' herein; dein Journal transkribiert sie.

journal.cli.transcribe.selection.conflict = { $all_flag } und { $input } schließen sich gegenseitig aus

journal.cli.transcribe.selection.missing = gib { $input } oder { $all_flag } an

journal.cli.transcribe.sound.all.windows.failed = Klangmarkierung ist für alle Fenster fehlgeschlagen: { $detail }

journal.cli.transcribe.sound.engine.absent = ced-Engine ist nicht installiert

journal.cli.transcribe.sound.failed = Klangmarkierung ist fehlgeschlagen: { $error }

journal.cli.transcribe.sound.helper.executable = ced-Helfer-Binärdatei ist nicht ausführbar: { $path }

journal.cli.transcribe.sound.helper.exit = ced-Helfer wurde mit { $code } beendet: { $stderr }

journal.cli.transcribe.sound.helper.io = I/O des ced-Helfers ist fehlgeschlagen: { $detail }

journal.cli.transcribe.sound.helper.large = ced-Antwort hat 64 MiB überschritten

journal.cli.transcribe.sound.helper.missing = ced-Helfer-Binärdatei fehlt: { $path }

journal.cli.transcribe.sound.helper.parent = aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.transcribe.sound.helper.response = Antwort des ced-Helfers war kein gültiges JSON: { $detail }

journal.cli.transcribe.sound.helper.spawn = ced-Helfer konnte nicht gestartet werden: { $detail }

journal.cli.transcribe.sound.helper.timeout = ced-Helfer wurde nicht innerhalb von { $timeout } beendet

journal.cli.transcribe.sound.helper.unresolved = ced-Helfer nicht aufgelöst: { $detail }

journal.cli.transcribe.sound.helper.windows = CED erfordert den Windows-Host als Aufrufer

journal.cli.transcribe.sound.package.platform = nicht unterstützte CED-Plattform { $os }/{ $arch }

journal.cli.transcribe.sound.package.windows.runtime = Windows-CED-Paketprüfung erfordert eine Windows-Laufzeitumgebung

journal.cli.transcribe.sound.prepare = Klangmarkierung konnte Audio nicht vorbereiten: { $error }

journal.cli.transcribe.sound.probe.helper.exit = ced-Sonden-Helfer wurde mit { $code } beendet: { $stderr }

journal.cli.transcribe.sound.probe.helper.unexpected = ced-Sonden-Helfer hat eine unerwartete Antwort zurückgegeben: { $response }

journal.cli.transcribe.sound.probe.unexpected = ced-Sonde hat eine unerwartete Antwort zurückgegeben: { $response }

journal.cli.transcribe.sound.schema = Klangmarkierung hat ein unerwartetes Antwortschema zurückgegeben

journal.cli.transcribe.sound.unsupported = Klangmarkierer deaktiviert: ced-Assets auf { $os }/{ $arch } nicht unterstützt

journal.cli.transcribe.sound.window.count = Klangmarkierung hat eine unerwartete Fensteranzahl zurückgegeben

journal.cli.transcribe.sound.window.ok = Klangmarkierung hat ein Fenster zurückgegeben, dem das Feld { $field } fehlt

journal.cli.transcribe.sound.windows.launch = Start des CED-Helfers ist fehlgeschlagen: { $error }

journal.cli.transcribe.sound.windows.library = CED-Anfragebibliothek stimmt nicht mit dem signierten Paket überein

journal.cli.transcribe.sound.windows.package.bin = laufende Programmdatei ist nicht im bin-Verzeichnis des Pakets

journal.cli.transcribe.sound.windows.package.member = signierte CED-App-Nutzdaten lehnen { $member } ab: { $error }

journal.cli.transcribe.sound.windows.package.parent = laufende Programmdatei hat kein umschließendes Verzeichnis

journal.cli.transcribe.sound.windows.package.root = Paket-bin-Verzeichnis hat keine Paketwurzel

journal.cli.transcribe.sound.windows.probe = CED-Helfer hat eine ungültige Sondenantwort zurückgegeben

journal.cli.transcribe.sound.windows.root = { $variable } ist nicht gesetzt

journal.cli.transcribe.sound.windows.signed = Windows-CED erfordert den signierten Paket-Helfer und seinen angegebenen Befehl

journal.cli.transcribe.stage.confidential.cap = Audio ist { $seconds } s und würde { $bytes } Bytes senden; vertrauliche STT akzeptiert höchstens { $limit } Bytes

journal.cli.transcribe.stage.confidential.verification.clear = Prüfung der vertraulichen Transkription wurde für { $path } nicht entfernt: { $error }

journal.cli.transcribe.stage.confidential.verification.record = Prüfung der vertraulichen Transkription wurde für { $path } nicht aufgezeichnet: { $error }

journal.cli.transcribe.stage.embedded.missing.draft = Erkennungslauf hat keinen Entwurfsspool

journal.cli.transcribe.stage.embedded.range.missing = zusammengeführter Wechselbereich fehlt

journal.cli.transcribe.stage.embedded.scratch = eingebetteter Scratch konnte nicht erstellt werden: { $error }

journal.cli.transcribe.stage.output.parent = Transkriptpfad hat kein übergeordnetes Verzeichnis

journal.cli.transcribe.stage.owner.deleted.unrecorded = Original nur mit Stimme gelöscht, aber das Löschereignis wurde nicht aufgezeichnet

journal.cli.transcribe.stage.owner.helper.outdated = der Helfer für die Sprecheranalyse ist älter als Überlappungsbereiche oder Sprecherwechsel

journal.cli.transcribe.stage.owner.not.deleted = Original nur mit Stimme jetzt nicht gelöscht: { $outcome }

journal.cli.transcribe.stage.owner.save.or.disable = speichere eine Stimme in Sprecher oder schalte { $setting } aus

journal.cli.transcribe.stage.owner.save.to.finish = speichere eine Stimme in Sprecher, um diese gefilterte Aufnahme abzuschließen

journal.cli.transcribe.summary = { $processed } verarbeitet, { $skipped } übersprungen (bereits transkribiert)

journal.cli.transcribe.summary.deferred = { $processed } verarbeitet, { $skipped } übersprungen (bereits transkribiert), { $deferred } zurückgestellt (Anbieter nicht bereit, wird wiederholt)

journal.cli.transcribe.summary.deferred.failed = { $processed } verarbeitet, { $skipped } übersprungen (bereits transkribiert), { $deferred } zurückgestellt (Anbieter nicht bereit, wird wiederholt), { $failed } fehlgeschlagen

journal.cli.transcribe.summary.failed = { $processed } verarbeitet, { $skipped } übersprungen (bereits transkribiert), { $failed } fehlgeschlagen

journal.cli.transcribe.usage.error = Aufruf: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $command }: Fehler: { $message }
