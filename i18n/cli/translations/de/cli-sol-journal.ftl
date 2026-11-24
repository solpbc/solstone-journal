journal.cli.journal.archive.argument = unerwartetes Argument

journal.cli.journal.archive.day.exclusive = { $day_flag } lässt sich nicht mit { $from_flag } oder { $to_flag } kombinieren

journal.cli.journal.archive.day.invalid = { $token_day } muss ein echtes { $token_yyyymmdd }-Datum sein

journal.cli.journal.archive.day.required = { $day_flag } erfordert { $token_yyyymmdd }

journal.cli.journal.archive.encode.finish.body = Rumpf des Archivabschlusses: { $detail }

journal.cli.journal.archive.encode.finish.finalize = endgültiger Archivabschluss: { $detail }

journal.cli.journal.archive.encode.member = Schreiben des Archivs { $member }: { $detail }

journal.cli.journal.archive.encode.metadata = ungültige Archivmetadaten { $field } { $value }: { $reason }

journal.cli.journal.archive.encode.source = Ursprung des Archivs: { $detail }

journal.cli.journal.archive.encode.write = Schreiben des Archivs: { $detail }

journal.cli.journal.archive.encode.writer = ungültiger Archivschreiber: { $reason }

journal.cli.journal.archive.export.help = Verwendung: { $syntax }{ $line_break }{ $line_break }erstelle ein { $token_zip } deines Journals, einschließlich Einstellungen und Zugangsdaten.{ $line_break }Verarbeitungsprotokolle, neu aufbaubare Indizes und Zwischenspeicher, Sperren und temporäre Dateien werden ausgelassen.{ $line_break }mit einem Datumsfilter sind nur die ausgewählten Tage enthalten.

journal.cli.journal.archive.exports.unsafe = unsicheres Exportverzeichnis: { $path }

journal.cli.journal.archive.fact.archive.output.contains.a.null.byte = die Archivausgabe enthält ein Nullbyte

journal.cli.journal.archive.fact.archive.output.ends.with.a.separator = die Archivausgabe endet mit einem Trennzeichen

journal.cli.journal.archive.fact.archive.output.has.no.file.name = die Archivausgabe hat keinen Dateinamen

journal.cli.journal.archive.fact.archive.output.has.no.parent = die Archivausgabe hat kein übergeordnetes Verzeichnis

journal.cli.journal.archive.fact.archive.output.must.be.one.file.name = die Archivausgabe muss genau ein Dateiname sein

journal.cli.journal.archive.fact.archive.output.parent.is.missing.or.unsafe = das übergeordnete Verzeichnis der Archivausgabe fehlt oder ist unsicher

journal.cli.journal.archive.fact.archive.output.path.is.not.valid.utf8 = der Pfad der Archivausgabe ist kein gültiges { $token_utf_8 }

journal.cli.journal.archive.fact.archive.output.uses.an.unsupported.prefix = die Archivausgabe verwendet ein nicht unterstütztes Präfix

journal.cli.journal.archive.fact.archive.parent.component.is.invalid = die übergeordnete Komponente des Archivs ist ungültig

journal.cli.journal.archive.fact.block.device = Blockgerät

journal.cli.journal.archive.fact.character.device = Zeichengerät

journal.cli.journal.archive.fact.could.not.determine.writer.position = die Position des Archivschreibers konnte nicht ermittelt werden

journal.cli.journal.archive.fact.could.not.read.writer.flags = die Flags des Archivschreibers konnten nicht gelesen werden

journal.cli.journal.archive.fact.could.not.stat.writer = der Archivschreiber konnte nicht mit stat erfasst werden

journal.cli.journal.archive.fact.directory = Verzeichnis

journal.cli.journal.archive.fact.fifo = FIFO

journal.cli.journal.archive.fact.filesystem.root.is.not.a.directory = die Dateisystemwurzel ist kein Verzeichnis

journal.cli.journal.archive.fact.has.no.parent.directory = hat kein übergeordnetes Verzeichnis

journal.cli.journal.archive.fact.must.be.a.regular.file = muss eine reguläre Datei sein

journal.cli.journal.archive.fact.must.be.empty = muss leer sein

journal.cli.journal.archive.fact.must.be.opened.for.writing = muss zum Schreiben geöffnet sein

journal.cli.journal.archive.fact.must.be.positioned.at.the.start = muss am Anfang stehen

journal.cli.journal.archive.fact.must.be.valid.utf8 = muss gültiges { $token_utf_8 } sein

journal.cli.journal.archive.fact.must.name.one.normal.file.leaf = muss genau ein normales Dateiblatt benennen

journal.cli.journal.archive.fact.must.not.be.opened.in.append.mode = darf nicht im Anhängemodus geöffnet sein

journal.cli.journal.archive.fact.must.resolve.to.an.absolute.path = muss zu einem absoluten Pfad aufgelöst werden

journal.cli.journal.archive.fact.other = Sonstiges

journal.cli.journal.archive.fact.regular.file = reguläre Datei

journal.cli.journal.archive.fact.relative.archive.output.requires.an.absolute.cwd = eine relative Archivausgabe erfordert ein absolutes Arbeitsverzeichnis

journal.cli.journal.archive.fact.socket = Socket

journal.cli.journal.archive.fact.symlink = Symlink

journal.cli.journal.archive.fact.utf8.member.path.exceeds.zip.u16.byte.limit = { $token_utf_8 }-Eintragspfad überschreitet die Byte-Grenze { $token_u16 } von { $token_zip }

journal.cli.journal.archive.from.invalid = { $token_from } muss ein echtes { $token_yyyymmdd }-Datum sein

journal.cli.journal.archive.from.required = { $from_flag } erfordert { $token_yyyymmdd }

journal.cli.journal.archive.import.ambiguities = Art stimmt nicht überein bei { $token_entities_ambiguities_jsonl }

journal.cli.journal.archive.import.appeared = { $relative } ist erschienen, während der Import lief

journal.cli.journal.archive.import.busy = die Zusammenführungssperre des Archivs ist bei { $path } belegt: { $detail }

journal.cli.journal.archive.import.cleanup = die Extraktion { $path } konnte nicht bereinigt werden: { $detail }

journal.cli.journal.archive.import.compressed = { $count ->
    [one] Archiv { $path } ist { $bytes } Byte; das Maximum ist { $maximum }
   *[other] Archiv { $path } ist { $bytes } Bytes; das Maximum ist { $maximum }
}

journal.cli.journal.archive.import.coordinate = ungültige geplante Aufnahmekoordinate

journal.cli.journal.archive.import.directory.remains = Verzeichnis verbleibt

journal.cli.journal.archive.import.encrypted.extraction = verschlüsselter Archiveintrag während der Extraktion: { $entry }

journal.cli.journal.archive.import.encrypted.validation = verschlüsselter Archiveintrag während der Validierung: { $entry }

journal.cli.journal.archive.import.entity = die Entität { $entity_id } konnte nicht zusammengeführt werden: { $detail }

journal.cli.journal.archive.import.expanded = { $count ->
    [one] Archiv wächst auf { $bytes } Byte an; das Maximum ist { $maximum }
   *[other] Archiv wächst auf { $bytes } Bytes an; das Maximum ist { $maximum }
}

journal.cli.journal.archive.import.extract = { $archive } konnte nicht in { $dir } extrahiert werden: { $detail }

journal.cli.journal.archive.import.facet = die Facette { $facet } konnte nicht zusammengeführt werden: { $detail }

journal.cli.journal.archive.import.handling = beim Bearbeiten von { $path }: { $detail }

journal.cli.journal.archive.import.hidden.archive = die ausgeblendeten Tage mit Körperdaten dieses Archivs können nicht gelesen werden, deshalb kann dieses Archiv nicht importiert werden

journal.cli.journal.archive.import.hidden.details = { $detail }; { $warning }

journal.cli.journal.archive.import.hidden.target = dieses Journal kann nicht lesen, welche Tage mit Körperdaten du ausgeblendet hast, deshalb kann dieses Archiv nicht importiert werden

journal.cli.journal.archive.import.hidden.warning = { $days ->
    [one] 1 Tag mit Körperdaten war im Archiv ausgeblendet, deshalb ist er auch in diesem Journal ausgeblendet; du kannst ihn auf der Körper-Seite wieder einblenden.
   *[other] { $days } Tage mit Körperdaten waren im Archiv ausgeblendet, deshalb sind sie auch in diesem Journal ausgeblendet; du kannst sie auf der Körper-Seite wieder einblenden.
}

journal.cli.journal.archive.import.invalid = ungültiges Archiv { $path }: { $detail }

journal.cli.journal.archive.import.kind = Art stimmt nicht überein bei { $relative }

journal.cli.journal.archive.import.legacy = ein { $token_tgz }- oder { $token_tar_gz }-Archiv wird nicht unterstützt. wenn es ein altes Transferarchiv ist, exportiere diesen Tag erneut als { $token_zip } aus dem Journal, aus dem er stammt, und führe das zusammen, oder führe diese Datei in ein neues, leeres Journal auf einem früheren { $token_2_x }-Release zusammen, exportiere dieses Journal als { $token_zip } und führe dieses { $token_zip } in dein Journal zusammen

journal.cli.journal.archive.import.lock = die Zusammenführungssperre des Archivs bei { $path } ist fehlgeschlagen: { $detail }

journal.cli.journal.archive.import.log = das Entscheidungsprotokoll bei { $path } konnte nicht geschrieben werden: { $detail }

journal.cli.journal.archive.import.magic = unerkanntes Magic des Archivs

journal.cli.journal.archive.import.marker = die Aktualisierung der Stream-Markierung ist fehlgeschlagen, nachdem der Archivinhalt eingespielt wurde: { $detail }; eingespielter Inhalt wurde nicht zurückgerollt

journal.cli.journal.archive.import.marker.day = { $day }: { $detail }

journal.cli.journal.archive.import.merge = die Zusammenführung des Imports bei { $path } ist fehlgeschlagen: { $detail }

journal.cli.journal.archive.import.missing = Archiv nicht gefunden: { $path }

journal.cli.journal.archive.import.name = Pfadname fehlt oder ist nicht { $token_utf_8 }

journal.cli.journal.archive.import.owner = warte auf den Halter oder untersuche oder entferne bei { $path } nur Eigentümer-Metadaten, die als veraltet geprüft sind

journal.cli.journal.archive.import.path.encoding = { $path } ist nicht { $token_utf_8 }

journal.cli.journal.archive.import.path.outside = { $path } liegt außerhalb des Journals

journal.cli.journal.archive.import.publish = das Einspielen der Archivzusammenführung ist fehlgeschlagen: { $detail }

journal.cli.journal.archive.import.redirect.entity = dieses Journal hat '{ $source_id }' in '{ $survivor }' zusammengeführt, und Zusammenführungen lassen sich nicht rückgängig machen, deshalb kann dieses Archiv nicht importiert werden

journal.cli.journal.archive.import.redirect.facet = dieses Journal hatte eine Facette namens '{ $facet }', die in eine andere zusammengeführt oder umbenannt wurde, und dieser Name führt jetzt dorthin, deshalb kann dieses Archiv nicht importiert werden

journal.cli.journal.archive.import.segment = das Segment bei { $path } konnte nicht zusammengeführt werden: { $detail }

journal.cli.journal.archive.import.segment.appeared = das Segment erschien vor dem Einspielen des Archivs

journal.cli.journal.archive.import.segment.changed = das eingespielte Archivsegment hat sich geändert

journal.cli.journal.archive.import.segment.symlink = Symlink im Segment

journal.cli.journal.archive.import.space = zu wenig freier Speicherplatz bei { $path }: { $available } verfügbar, { $required } erforderlich

journal.cli.journal.archive.import.stage = die Bereitstellung bei { $path } konnte nicht geschrieben werden: { $detail }

journal.cli.journal.archive.import.stage.changed = das übergeordnete Verzeichnis der Archivbereitstellung hat sich geändert

journal.cli.journal.archive.import.stopped = Import gestoppt

journal.cli.journal.archive.import.symlink = Symlink

journal.cli.journal.archive.import.traversal = absoluter Pfad oder Verzeichnisaufstieg

journal.cli.journal.archive.import.unambiguous = das Archiv hat keine eindeutige Journal-Wurzel

journal.cli.journal.archive.import.undo = { $detail }; Rückgängigmachen unvollständig: { $failures }

journal.cli.journal.archive.import.undo.coordinate = Koordinate { $relative }: { $detail }

journal.cli.journal.archive.import.undo.restore = Wiederherstellen { $relative }: { $detail }

journal.cli.journal.archive.import.undo.rewrite = Identitätszuordnung neu schreiben: { $detail }

journal.cli.journal.archive.import.undo.unlink = unlink { $relative }: { $detail }

journal.cli.journal.archive.import.unimplemented = import-sources: nicht implementiert: { $module }

journal.cli.journal.archive.import.unsafe.extraction = unsicherer Archiveintrag während der Extraktion: { $entry }: { $detail }

journal.cli.journal.archive.import.unsafe.validation = unsicherer Archiveintrag während der Validierung: { $entry }: { $detail }

journal.cli.journal.archive.incomplete = { $committed } übernommen, { $skipped } übersprungen, { $failed } fehlgeschlagen

journal.cli.journal.archive.ios = nicht verfügbar auf { $token_ios }

journal.cli.journal.archive.merge.help = Verwendung: { $syntax }

journal.cli.journal.archive.metadata.reason.invalid.calendar.date.or.time = ungültiges Kalenderdatum oder ungültige Uhrzeit

journal.cli.journal.archive.metadata.reason.invalid.day = ungültiger Tag

journal.cli.journal.archive.metadata.reason.invalid.hour = ungültige Stunde

journal.cli.journal.archive.metadata.reason.invalid.minute = ungültige Minute

journal.cli.journal.archive.metadata.reason.invalid.month = ungültiger Monat

journal.cli.journal.archive.metadata.reason.invalid.second = ungültige Sekunde

journal.cli.journal.archive.metadata.reason.must.contain.digits.in.numeric.positions = muss Ziffern an numerischen Positionen enthalten

journal.cli.journal.archive.metadata.reason.must.not.be.empty = darf nicht leer sein

journal.cli.journal.archive.metadata.reason.must.not.contain.ascii.control.characters = darf keine ASCII-Steuerzeichen enthalten

journal.cli.journal.archive.metadata.reason.must.use.yyyy.mm.ddthh.mm.ssz = muss YYYY-MM-DDTHH:MM:SSZ verwenden

journal.cli.journal.archive.metadata.reason.second.must.be.in.seconds = Sekunde muss in 0..=59 liegen

journal.cli.journal.archive.metadata.reason.year.must.be.in.years = Jahr muss in 1980..=2107 liegen

journal.cli.journal.archive.negative.identity = die Identität des Ursprungs ist negativ

journal.cli.journal.archive.negative.size = die Größe der regulären Datei ist negativ

journal.cli.journal.archive.out.required = { $out_flag } erfordert { $token_path }

journal.cli.journal.archive.output.parent = die Archivausgabe hat kein übergeordnetes Verzeichnis

journal.cli.journal.archive.output.tree = die Ausgabe liegt im exportierten { $family }-Baum

journal.cli.journal.archive.preview = Archivzusammenführung (Tage: { $days }): 0 übernommen, { $skipped } übersprungen, 0 fehlgeschlagen.

journal.cli.journal.archive.publish.cleanup = Bereinigen der temporären Archivdatei: { $detail }

journal.cli.journal.archive.publish.createtemp = Erstellen der temporären Archivdatei: { $detail }

journal.cli.journal.archive.publish.publish = Einspielen des Archivs: { $detail }

journal.cli.journal.archive.publish.syncdirectory = Abgleich des Archivverzeichnisses: { $detail }

journal.cli.journal.archive.publish.synctemp = Abgleich der temporären Archivdatei: { $detail }

journal.cli.journal.archive.range = { $from_flag } darf nicht nach { $to_flag } liegen

journal.cli.journal.archive.root.name = die Journal-Wurzel hat keinen Dateinamen

journal.cli.journal.archive.root.parent = die Journal-Wurzel hat kein übergeordnetes Verzeichnis

journal.cli.journal.archive.scratch.name = eine exklusive Scratch-Datei für das Archiv konnte nicht bereitgestellt werden

journal.cli.journal.archive.skipped = { $token_solstone_journal_archive_export }: übersprungene Einträge der obersten Ebene: { $skipped }

journal.cli.journal.archive.source.budget = nicht unterstützte Journal-Wurzel { $path }: der Ursprung des Archivs überschreitet sein unterstütztes Inventarbudget

journal.cli.journal.archive.source.changed = der Ursprung des Journals hat sich bei { $member } geändert

journal.cli.journal.archive.source.io.list.journal.directory = Auflisten des Journal-Verzeichnisses: { $detail }

journal.cli.journal.archive.source.io.list.journal.directory.member = Auflisten des Journal-Verzeichnisses { $member }: { $detail }

journal.cli.journal.archive.source.io.member = { $operation } { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing = Öffnen des Journal-Verzeichnisses zum Auflisten: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing.member = Öffnen des Journal-Verzeichnisses zum Auflisten { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.member = Öffnen des Journal-Verzeichnisses { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.file = Öffnen der Journal-Datei: { $detail }

journal.cli.journal.archive.source.io.open.journal.file.member = Öffnen der Journal-Datei { $member }: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root = Öffnen der behaltenen Journal-Wurzel: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root.member = Öffnen der behaltenen Journal-Wurzel { $member }: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file = Lesen der inventarisierten Datei: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file.member = Lesen der inventarisierten Datei { $member }: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size = Lesen der Größe der regulären Datei: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size.member = Lesen der Größe der regulären Datei { $member }: { $detail }

journal.cli.journal.archive.source.io.read.source.file.identity.member = Lesen der Identität der Ursprungsdatei { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory = stat des inventarisierten Verzeichnisses: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory.member = stat des inventarisierten Verzeichnisses { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file = stat der inventarisierten Datei: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file.member = stat der inventarisierten Datei { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry = stat des Journal-Eintrags: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry.member = stat des Journal-Eintrags { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory = stat des geöffneten inventarisierten Verzeichnisses: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory.member = stat des geöffneten inventarisierten Verzeichnisses { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file = stat der geöffneten inventarisierten Datei: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file.member = stat der geöffneten inventarisierten Datei { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory = stat des geöffneten Journal-Verzeichnisses: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory.member = stat des geöffneten Journal-Verzeichnisses { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file = stat der geöffneten Journal-Datei: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file.member = stat der geöffneten Journal-Datei { $member }: { $detail }

journal.cli.journal.archive.source.missing = { $token_source } existiert nicht: { $path }

journal.cli.journal.archive.source.regular = { $token_source } muss eine reguläre Datei sein

journal.cli.journal.archive.source.required = { $token_source } ist erforderlich

journal.cli.journal.archive.source.unsafe = unsicherer Journal-Eintrag { $member }: { $kind }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness = rekursiver ARM-Namespace-Witness: { $detail }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness.member = rekursiver ARM-Namespace-Witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root = Ausleihen der zugelassenen Journal-Wurzel: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.member = Ausleihen der zugelassenen Journal-Wurzel { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch = Ausleihen der zugelassenen Namespace-Überwachung der Journal-Wurzel: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch.member = Ausleihen der zugelassenen Namespace-Überwachung der Journal-Wurzel { $member }: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness = Abbrechen des rekursiven Namespace-Witness: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness.member = Abbrechen des rekursiven Namespace-Witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness = Prüfen des rekursiven Namespace-Witness: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness.member = Prüfen des rekursiven Namespace-Witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event = Erstellen des Ereignisses für den rekursiven Namespace-Witness: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event.member = Erstellen des Ereignisses für den rekursiven Namespace-Witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness = Leeren des rekursiven Namespace-Witness: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness.member = Leeren des rekursiven Namespace-Witness { $member }: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory = Auflisten des behaltenen Verzeichnisses: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory.member = Auflisten des behaltenen Verzeichnisses { $member }: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry = Öffnen des behaltenen relativen Eintrags: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry.member = Öffnen des behaltenen relativen Eintrags { $member }: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing = Parsen der behaltenen Verzeichnisauflistung: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing.member = Parsen der behaltenen Verzeichnisauflistung { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes = Abfragen der Attribute des behaltenen Eintrags: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes.member = Abfragen der Attribute des behaltenen Eintrags { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity = Abfragen der Identität des behaltenen Eintrags: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity.member = Abfragen der Identität des behaltenen Eintrags { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata = Abfragen der Metadaten des behaltenen Eintrags: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata.member = Abfragen der Metadaten des behaltenen Eintrags { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type = Abfragen des Typs des behaltenen Eintrags: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type.member = Abfragen des Typs des behaltenen Eintrags { $member }: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file = Lesen der beobachteten Datei: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file.member = Lesen der beobachteten Datei { $member }: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity = erneutes Überprüfen der Identität des behaltenen relativen Verzeichnisses: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity.member = erneutes Überprüfen der Identität des behaltenen relativen Verzeichnisses { $member }: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer = Größenbestimmung des Puffers für geprüftes Lesen: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer.member = Größenbestimmung des Puffers für geprüftes Lesen { $member }: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open = Nachverfolgen des Öffnens der behaltenen relativen Verzeichnisauflistung: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open.member = Nachverfolgen des Öffnens der behaltenen relativen Verzeichnisauflistung { $member }: { $detail }

journal.cli.journal.archive.source.windows.unsupported.arm.recursive.namespace.witness = nicht unterstützte Journal-Wurzel { $path }: rekursiver ARM-Namespace-Witness

journal.cli.journal.archive.source.windows.unsupported.cancel.recursive.namespace.witness = nicht unterstützte Journal-Wurzel { $path }: Abbrechen des rekursiven Namespace-Witness

journal.cli.journal.archive.source.windows.unsupported.check.recursive.namespace.witness = nicht unterstützte Journal-Wurzel { $path }: Prüfen des rekursiven Namespace-Witness

journal.cli.journal.archive.source.windows.unsupported.drain.recursive.namespace.witness = nicht unterstützte Journal-Wurzel { $path }: Leeren des rekursiven Namespace-Witness

journal.cli.journal.archive.source.windows.unsupported.wait.for.recursive.namespace.witness = nicht unterstützte Journal-Wurzel { $path }: Warten auf den rekursiven Namespace-Witness

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness = Warten auf den rekursiven Namespace-Witness: { $detail }

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness.member = Warten auf den rekursiven Namespace-Witness { $member }: { $detail }

journal.cli.journal.archive.space.overflow = Überlauf des verfügbaren Speicherplatzes

journal.cli.journal.archive.space.platform = die Speicherplatzprüfung ist auf dieser Plattform nicht unterstützt

journal.cli.journal.archive.summary = Archivzusammenführung: { $committed } übernommen, { $skipped } übersprungen, { $failed } fehlgeschlagen.

journal.cli.journal.archive.target.changed = der Weg der Archivausgabe hat sich während des Exports geändert: { $path }

journal.cli.journal.archive.target.collision = die Archivausgabe ist bereits vorhanden: { $path }

journal.cli.journal.archive.target.invalid = ungültige Archivausgabe { $path }: { $reason }

journal.cli.journal.archive.target.io = { $operation } für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.filesystem.root = Öffnen der Dateisystemwurzel der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.parent = Öffnen des übergeordneten Verzeichnisses der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.retain.admitted.archive.output.parent = Behalten des zugelassenen übergeordneten Verzeichnisses der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.revalidate.admitted.archive.output.parent = erneutes Validieren des zugelassenen übergeordneten Verzeichnisses der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.stat.admitted.archive.output.parent = stat des zugelassenen übergeordneten Verzeichnisses der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.filesystem.root = stat der Dateisystemwurzel der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.final.name = stat des endgültigen Namens der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.parent = stat des übergeordneten Verzeichnisses der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.stat.opened.archive.output.parent = stat des geöffneten übergeordneten Verzeichnisses der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.io.stat.retained.archive.output.parent = stat des behaltenen übergeordneten Verzeichnisses der Archivausgabe für { $path } fehlgeschlagen: { $detail }

journal.cli.journal.archive.target.unsafe = die Archivausgabe ist unsicher ({ $kind }): { $path }

journal.cli.journal.archive.target.windows.changed = die Archivausgabe hat sich geändert: { $path }

journal.cli.journal.archive.target.windows.io = Archivausgabe { $path }: { $detail }

journal.cli.journal.archive.target.windows.unsafe = unsichere Archivausgabe { $path }: { $reason }

journal.cli.journal.archive.temp.name = ein eindeutiger temporärer Name für das Archiv konnte nicht bereitgestellt werden

journal.cli.journal.archive.to.invalid = { $token_to } muss ein echtes { $token_yyyymmdd }-Datum sein

journal.cli.journal.archive.to.required = { $to_flag } erfordert { $token_yyyymmdd }

journal.cli.journal.archive.zero.write = der Archivschreiber hat für ein nicht leeres Schreiben null Bytes zurückgegeben

journal.cli.journal.facet.argument.ios = auf { $platform } nicht verfügbar

journal.cli.journal.facet.argument.unexpected = unerwartetes Argument

journal.cli.journal.facet.content.unsafe = unsicherer Facetteninhalt: { $path }

journal.cli.journal.facet.doctor.adopted.heading = Übernommene verwaiste Facetten

journal.cli.journal.facet.doctor.adoption.failed = { $destination } (Übernahme fehlgeschlagen: { $error })

journal.cli.journal.facet.doctor.adoption.unmerged = { $folders_remaining ->
    [one] { $destination } (Übernahme fehlgeschlagen: { $error }; { $unmerged } wurde nicht zusammengeführt)
   *[other] { $destination } (Übernahme fehlgeschlagen: { $error }; { $unmerged } wurden nicht zusammengeführt)
    }

journal.cli.journal.facet.doctor.argument.merge = { $merge_flag } erfordert { $adopt_flag }

journal.cli.journal.facet.doctor.collision.heading = Kollisionen regulärer Dateien

journal.cli.journal.facet.doctor.empty = Nichts zu melden.

journal.cli.journal.facet.doctor.failed.heading = Fehlgeschlagene verwaiste Facetten

journal.cli.journal.facet.doctor.fix.ios = { $fix_flag } ist auf { $platform } nicht verfügbar

journal.cli.journal.facet.doctor.folders.hidden = Ordner, deren Namen mit einem Punkt beginnen, sind nie Facetten, daher werden diese Facettendeklarationen ignoriert: { $folders }

journal.cli.journal.facet.doctor.group.ambiguous = { $members }: benannt wie { $facets }; benenne den Ordner so um, dass er zu einer passt, und führe es dann mit { $adopt_flag } { $merge_flag } aus

journal.cli.journal.facet.doctor.group.damaged = { $members }: benannt wie { $facets }, deren { $declaration } zuerst repariert werden muss

journal.cli.journal.facet.doctor.help = Verwendung: { $syntax }{ $line_break }{ $retire_syntax }

journal.cli.journal.facet.doctor.history.failed = Scan des Aktionsprotokolls fehlgeschlagen: { $error }

journal.cli.journal.facet.doctor.history.merged = { $name } wurde in { $destination } zusammengeführt

journal.cli.journal.facet.doctor.history.recorded = Aus den Aktionsprotokollen aufgezeichnete stillgelegte Facettennamen:

journal.cli.journal.facet.doctor.history.renamed = { $name } wurde in { $destination } umbenannt

journal.cli.journal.facet.doctor.history.reused = Diese Facetten wurden früher umbenannt oder durch Zusammenführung entfernt, und ihre Namen wurden später einer neuen Facette gegeben, daher lässt sich älteres Material, das sie nennt, nicht auseinanderhalten. Es wurde nichts geändert: { $names }

journal.cli.journal.facet.doctor.history.unknown = Die Aktionsprotokolle erwähnen diese Namen, aber nicht, wohin sie gegangen sind: { $names }. Wenn Material noch einen nennt, zeichne ihn mit { $retire_arguments } oder { $deleted_flag } auf.

journal.cli.journal.facet.doctor.history.unrecorded = Facettennamen, die laut den Aktionsprotokollen stillgelegt wurden, aber nicht aufgezeichnet sind (führe es mit { $fix_flag } aus, um sie aufzuzeichnen):

journal.cli.journal.facet.doctor.leftovers.failed = Zusammenführungsreste konnten nicht gelesen werden: { $error }

journal.cli.journal.facet.doctor.leftovers.kept = Restordner einer Facettenzusammenführung, die die einzige Kopie einiger Dateien halten können. Lösche sie nicht; führe es mit { $fix_flag } aus, um sie in den Papierkorb zu verschieben: { $folders }

journal.cli.journal.facet.doctor.leftovers.moved = Restordner einer Facettenzusammenführung so, wie sie waren, in den Papierkorb verschoben: { $folders }

journal.cli.journal.facet.doctor.leftovers.safe = Restordner einer unterbrochenen Facettenzusammenführung, die gelöscht werden können, sobald keine Zusammenführung läuft: { $folders }

journal.cli.journal.facet.doctor.link.duplicate.open = { $facet }: '{ $id }' ist von mehr als einem Ordner verknüpft ({ $folders })

journal.cli.journal.facet.doctor.link.duplicate.repaired = { $facet }: '{ $id }' war von mehr als einem Ordner verknüpft ({ $folders })

journal.cli.journal.facet.doctor.link.merged.open = { $folder_count ->
    [one] { $facet }: '{ $id }' wurde in '{ $successor }' zusammengeführt, und der Ordner { $folders } verknüpft es noch
   *[other] { $facet }: '{ $id }' wurde in '{ $successor }' zusammengeführt, und die Ordner { $folders } verknüpfen es noch
    }

journal.cli.journal.facet.doctor.link.merged.repaired = { $folder_count ->
    [one] { $facet }: '{ $id }' wurde in '{ $successor }' zusammengeführt, und der Ordner { $folders } verknüpfte es
   *[other] { $facet }: '{ $id }' wurde in '{ $successor }' zusammengeführt, und die Ordner { $folders } verknüpften es
    }

journal.cli.journal.facet.doctor.link.misnamed.open = { $facet }: '{ $id }' ist vom Ordner { $folders } verknüpft, nicht von einem Ordner namens '{ $id }'

journal.cli.journal.facet.doctor.link.misnamed.repaired = { $facet }: '{ $id }' war vom Ordner { $folders } verknüpft, nicht von einem Ordner namens '{ $id }'

journal.cli.journal.facet.doctor.link.unreadable = { $facet }: Ordner { $folders } hat eine Verknüpfung, die nicht gelesen oder verwendet werden kann

journal.cli.journal.facet.doctor.link.unreadable.check = { $facet }: Ordner { $folders } hat eine Verknüpfung, die nicht gelesen oder verwendet werden kann; { $fix_flag } lässt sie so

journal.cli.journal.facet.doctor.link.unreadable.undecided = { $facet }: Ordner { $folders } hat eine Verknüpfung, die nicht gelesen oder verwendet werden kann ({ $fix_flag } kann nicht erkennen, welche Entität sie verknüpft, und lässt sie daher)

journal.cli.journal.facet.doctor.links.check = Entitätsverknüpfungen zum Reparieren (führe es mit { $fix_flag } aus, um die zu reparieren, die sich reparieren lassen):

journal.cli.journal.facet.doctor.links.check.failed = Prüfung der Entitätsverknüpfungen fehlgeschlagen: { $error }

journal.cli.journal.facet.doctor.links.left = So gelassene Entitätsverknüpfungen:

journal.cli.journal.facet.doctor.links.repair.failed = Reparatur der Entitätsverknüpfungen fehlgeschlagen: { $error }

journal.cli.journal.facet.doctor.links.repaired = Reparierte Entitätsverknüpfungen:

journal.cli.journal.facet.doctor.links.rescan = Führe '{ $rescan_command }' aus, um den Index aufzufrischen.

journal.cli.journal.facet.doctor.lock.failed = die Facettensperre wurde nicht erlangt: { $error }

journal.cli.journal.facet.doctor.maintenance.heading = Festgeschriebene Wartungsfehler bei Zusammenführungen

journal.cli.journal.facet.doctor.merge.collision = { $source } -> { $destination }: { $path } ({ $retained } behalten)

journal.cli.journal.facet.doctor.merge.failed = { $source } -> { $destination } (Zusammenführung vor dem Festschreiben fehlgeschlagen: { $error })

journal.cli.journal.facet.doctor.merge.ios = Zusammenführen von Facetten auf { $platform } nicht verfügbar

journal.cli.journal.facet.doctor.merged.heading = Zusammengeführte verwaiste Facetten

journal.cli.journal.facet.doctor.merged.none = Keine verwaiste Facette wurde registriert oder zusammengeführt.

journal.cli.journal.facet.doctor.merges.incomplete = eine oder mehrere Zusammenführungen verwaister Facetten wurden mit Wartungsfehlern festgeschrieben

journal.cli.journal.facet.doctor.orphans.count = { $count ->
    [one] { $count } verwaiste Facette gefunden. Führe es mit { $adopt_flag } aus, um sie zu registrieren.
   *[other] { $count } verwaiste Facetten gefunden. Führe es mit { $adopt_flag } aus, um sie zu registrieren.
    }

journal.cli.journal.facet.doctor.orphans.heading = Verwaiste Facetten:

journal.cli.journal.facet.doctor.orphans.held = Die Aktionsprotokolle zeigen, dass diese Facettennamen stillgelegt wurden, daher sind ihre Ordner nicht registriert; führe es mit { $fix_flag } aus, um sie aufzuzeichnen: { $folders }

journal.cli.journal.facet.doctor.orphans.none = Keine verwaisten Facetten gefunden.

journal.cli.journal.facet.doctor.orphans.retired = Diese Ordner haben Inhalt unter Facettennamen, die in eine andere Facette zusammengeführt oder umbenannt wurden, daher sind sie nicht als Facetten registriert; ihre Dateien bleiben an Ort und Stelle: { $folders }

journal.cli.journal.facet.doctor.queue.failed = die Reparatur ist fertig, aber ihre Suchaktualisierung konnte nicht in die Warteschlange gestellt werden: { $error }

journal.cli.journal.facet.doctor.reference = - { $name }: { $count } ({ $first } bis { $last })

journal.cli.journal.facet.doctor.references.command = Um eine wieder erreichbar zu machen, führe aus: { $retire_command }

journal.cli.journal.facet.doctor.references.failed = Referenzscan fehlgeschlagen: { $error }

journal.cli.journal.facet.doctor.references.missing = Segmente nennen diese Facetten, die nicht existieren und nicht als stillgelegt aufgezeichnet sind. Agenten, die auf gewählte Facetten beschränkt sind, können dieses Material nicht sehen; alles andere kann es:

journal.cli.journal.facet.doctor.registered.none = Keine verwaiste Facette wurde registriert.

journal.cli.journal.facet.doctor.registration.failed = { $slug } wurde registriert, aber der Datensatz konnte nicht geschrieben werden: { $error }

journal.cli.journal.facet.doctor.repair.failed = stillgelegte Facettennamen konnten nicht repariert werden: { $error }

journal.cli.journal.facet.doctor.repaired.count = { $repaired ->
    [one] { $repaired } verwaiste Facette repariert. die Suche holt auf.
   *[other] { $repaired } verwaiste Facetten repariert. die Suche holt auf.
    }

journal.cli.journal.facet.doctor.repaired.heading = Reparierte verwaiste Facetten:

journal.cli.journal.facet.doctor.repairs.failed = eine oder mehrere Reparaturen verwaister Facetten sind fehlgeschlagen

journal.cli.journal.facet.doctor.repairs.failed.count = { $repaired ->
    [one] { $failed_orphans ->
        [one] { $repaired } verwaiste Facette repariert; { $failed_orphans } verwaiste Facette fehlgeschlagen. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
       *[other] { $repaired } verwaiste Facette repariert; { $failed_orphans } verwaiste Facetten fehlgeschlagen. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
        }
   *[other] { $failed_orphans ->
        [one] { $repaired } verwaiste Facetten repariert; { $failed_orphans } verwaiste Facette fehlgeschlagen. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
       *[other] { $repaired } verwaiste Facetten repariert; { $failed_orphans } verwaiste Facetten fehlgeschlagen. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
        }
    }

journal.cli.journal.facet.doctor.repairs.maintenance.count = { $repaired ->
    [one] { $merges ->
        [one] { $repaired } verwaiste Facette repariert; { $merges } Zusammenführung wurde festgeschrieben, hat aber nach dem Festschreiben einen Wartungsfehler gemeldet. Siehe 'Festgeschriebene Wartungsfehler bei Zusammenführungen' weiter oben. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
       *[other] { $repaired } verwaiste Facette repariert; { $merges } Zusammenführungen wurden festgeschrieben, haben aber nach dem Festschreiben einen Wartungsfehler gemeldet. Siehe 'Festgeschriebene Wartungsfehler bei Zusammenführungen' weiter oben. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
        }
   *[other] { $merges ->
        [one] { $repaired } verwaiste Facetten repariert; { $merges } Zusammenführung wurde festgeschrieben, hat aber nach dem Festschreiben einen Wartungsfehler gemeldet. Siehe 'Festgeschriebene Wartungsfehler bei Zusammenführungen' weiter oben. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
       *[other] { $repaired } verwaiste Facetten repariert; { $merges } Zusammenführungen wurden festgeschrieben, haben aber nach dem Festschreiben einen Wartungsfehler gemeldet. Siehe 'Festgeschriebene Wartungsfehler bei Zusammenführungen' weiter oben. Führe '{ $rescan_command }' aus, um den Index aufzufrischen.
        }
    }

journal.cli.journal.facet.doctor.retired.malformed = { $retired_path } lässt sich nicht parsen ({ $detail }). Führe es mit { $fix_flag } aus, um den Pfad neu aufzubauen; kein Name, den der Pfad reserviert, wird freigegeben.

journal.cli.journal.facet.doctor.retired.rebuilt = { $retired_path } ließ sich nicht parsen ({ $detail }); nach { $path } verschoben und neu aufgebaut.

journal.cli.journal.facet.doctor.retired.reserved = Diese Namen konnten nicht nachverfolgt werden und sind als gelöscht aufgezeichnet: { $names }.

journal.cli.journal.facet.doctor.retired.unreadable = { $retired_path } konnte nicht gelesen werden ({ $detail }). Prüfe die Berechtigungen; der Pfad wurde so gelassen.

journal.cli.journal.facet.doctor.search.failed = Suchaktualisierung konnte nicht in die Warteschlange gestellt werden: { $error }

journal.cli.journal.facet.doctor.search.queued = die Suche holt die Facettenänderungen auf.

journal.cli.journal.facet.doctor.search.skipped = Die Suche wurde nicht aktualisiert, weil { $retired_path } noch nicht verwendet werden kann.

journal.cli.journal.facet.doctor.twins.heading = Ordner, die wie eine Facette heißen, die du hast:

journal.cli.journal.facet.doctor.twins.merge = Diese werden nie neben ihr registriert. Führe es mit { $adopt_flag } { $merge_flag } aus, um sie in diese Facette zusammenzuführen; eine Zusammenführung ist endgültig, und bei einer Datei, die beide haben, wird die Kopie der Facette behalten.

journal.cli.journal.facet.doctor.twins.preview = Um zuerst zu sehen, was einer einbringen würde: { $merge_command } { $source } { $into_flag } { $destination } { $dry_run_flag }

journal.cli.journal.facet.doctor.unclear.heading = So gelassene Ordner

journal.cli.journal.facet.doctor.variants.heading = Gruppen von Namensvarianten:

journal.cli.journal.facet.doctor.variants.merge = Diese wurden nicht registriert. Führe es mit { $adopt_flag } { $merge_flag } aus, um sie zu einer zusammenzulegen.

journal.cli.journal.facet.doctor.variants.register = Führe es mit { $adopt_flag } { $merge_flag } aus, um Namensvarianten zusammenzulegen, bevor sie registriert werden.

journal.cli.journal.facet.entry.unsafe = unsicherer Facetteneintrag: { $path }

journal.cli.journal.facet.hidden.damaged = die verborgenen Namen unter { $path } können nicht gelesen werden, daher wurde nichts geändert: { $detail }

journal.cli.journal.facet.hidden.empty = es wurde kein Name angegeben

journal.cli.journal.facet.hidden.field.text = { $key } ist kein Text

journal.cli.journal.facet.hidden.field.unknown = unbekanntes Feld { $key }

journal.cli.journal.facet.hidden.names.list = { $names_field } ist keine Liste

journal.cli.journal.facet.hidden.row.empty = Name ist leer

journal.cli.journal.facet.hidden.row.error = Name { $index }: { $detail }

journal.cli.journal.facet.hidden.row.object = kein Objekt

journal.cli.journal.facet.json.object = { $path } ist kein { $json_format }-Objekt

journal.cli.journal.facet.link.conflict = { $path } hat in den Ordnern, die kombiniert werden, unterschiedliche Inhalte, daher wurde nichts kombiniert

journal.cli.journal.facet.link.file.encoding = { $path } ist kein { $encoding }-Text

journal.cli.journal.facet.link.ids.exhausted = Notizen haben keine ID-Nummern mehr

journal.cli.journal.facet.link.invalid.name = '{ $name }' kann nicht als Entitäts-ID verwendet werden

journal.cli.journal.facet.link.not.file = { $path } ist ein Ordner oder ein Symlink, keine Datei, daher wurde nichts kombiniert

journal.cli.journal.facet.link.observation.corrupt = korrupte Beobachtung in { $source }: { $detail }

journal.cli.journal.facet.link.observation.malformed = fehlerhafte Beobachtung in { $source }, Zeile { $line }: { $reason }

journal.cli.journal.facet.link.occupied = Ordner '{ $id }' hält eine andere Entität oder kann nicht gelesen werden

journal.cli.journal.facet.link.repair.destination = der Ordner für '{ $id }' in der Facette, in die zusammengeführt wird, hält eine andere Entität oder kann nicht gelesen werden, daher wurde nichts geändert; führe '{ $doctor_command }' aus, um zu sehen, was repariert werden muss

journal.cli.journal.facet.link.repair.named = der Ordner für '{ $id }' in der Facette '{ $facet }' hält eine andere Entität oder kann nicht gelesen werden, daher wurde nichts geändert; führe '{ $doctor_command }' aus, um zu sehen, was repariert werden muss

journal.cli.journal.facet.link.successor.missing = die Entität, der beigetreten wurde, kann nicht gefunden werden

journal.cli.journal.facet.log.action.queue = Suchaktualisierung des Aktionsprotokolls konnte nicht in die Warteschlange gestellt werden: { $error }

journal.cli.journal.facet.log.detections.queue = Facettenerkennungen nicht für die Suche in die Warteschlange gestellt facet={ $facet } cause={ $error }

journal.cli.journal.facet.log.trash.unfinished = Papierkorbelement { $id }: verschoben, wird später fertig: { $error }

journal.cli.journal.facet.merge.argument.confirm = eine Zusammenführung ist endgültig und lässt sich nicht rückgängig machen; prüfe sie mit { $dry_run_flag } und führe sie dann erneut mit { $yes_flag } aus

journal.cli.journal.facet.merge.argument.dest = { $into_flag } erfordert { $destination_argument }

journal.cli.journal.facet.merge.argument.into = { $into_dest } ist erforderlich

journal.cli.journal.facet.merge.argument.invalid = { $source_argument } oder { $destination_argument } ist ungültig

journal.cli.journal.facet.merge.argument.source = { $source_argument } ist erforderlich

journal.cli.journal.facet.merge.cleanup.failed = die Zusammenführung ist fertig, aber ihre Restordner konnten nicht entfernt werden: { $error }; führe '{ $repair_command }' aus, um sie aufzuräumen

journal.cli.journal.facet.merge.collisions.kept = Beide Facetten hatten diese Dateien. '{ $destination }' hat die eigene Kopie behalten; die Kopie in '{ $source }' wurde behalten, und '{ $repair_command }' verschiebt sie in den Papierkorb:

journal.cli.journal.facet.merge.collisions.trashed = Beide Facetten hatten diese Dateien. '{ $destination }' hat die eigene Kopie behalten; die Kopie in '{ $source }' ist im Papierkorb:

journal.cli.journal.facet.merge.complete = Zusammengeführt: '{ $source }' in '{ $destination }'. Neuaufbau des Index abgeschlossen.

journal.cli.journal.facet.merge.destination.absent = '{ $destination }' ist keine Facette

journal.cli.journal.facet.merge.destination.id = '{ $destination }' hat noch keine stabile ID; führe zuerst '{ $backfill_command }' aus

journal.cli.journal.facet.merge.destination.retired = '{ $destination }' ist als stillgelegter Facettenname aufgezeichnet; führe '{ $doctor_command }' für die Details aus

journal.cli.journal.facet.merge.detections.failed = die Erkennungen wurden nicht in die Warteschlange gestellt

journal.cli.journal.facet.merge.failed = Facettenzusammenführung fehlgeschlagen

journal.cli.journal.facet.merge.help = Verwendung: { $syntax }

journal.cli.journal.facet.merge.preview.empty = Nichts.

journal.cli.journal.facet.merge.preview.entities = { $count ->
    [one] { $count } Entität ist in beiden Facetten verknüpft; sie würde mit einer Verknüpfung enden und jede Notiz behalten.
   *[other] { $count } Entitäten sind in beiden Facetten verknüpft; jede würde mit einer Verknüpfung enden und jede Notiz behalten.
    }

journal.cli.journal.facet.merge.preview.fields = Entitätsfelder, bei denen '{ $destination }' den eigenen, abweichenden Wert behalten würde:

journal.cli.journal.facet.merge.preview.files = Dateien, die beide Facetten haben und die sich nicht kombinieren lassen. '{ $destination }' würde die eigene Kopie behalten, und die Kopie in '{ $source }' würde in den Papierkorb kommen:

journal.cli.journal.facet.merge.preview.heading = Probelauf, es wurde nichts geändert. Das Zusammenführen von '{ $source }' in '{ $destination }' würde verlieren:

journal.cli.journal.facet.merge.preview.notes = { $count ->
    [one] { $count } Notiz würde eine neue Nummer bekommen, nach den Notizen, die schon da sind.
   *[other] { $count } Notizen würden eine neue Nummer bekommen, nach den Notizen, die schon da sind.
    }

journal.cli.journal.facet.merge.preview.records = Datensätze, die verworfen würden, weil ein anderer Datensatz mit derselben ID behalten wird (der erste Datensatz mit jeder ID wird behalten, und '{ $destination }' wird zuerst gelesen):

journal.cli.journal.facet.merge.preview.settings = Die Einstellungen in { $declaration } von '{ $source }', die nicht übernommen würden.

journal.cli.journal.facet.merge.queue.failed = die Reparatur ist fertig, aber ihre Suchaktualisierung konnte nicht in die Warteschlange gestellt werden: { $error }

journal.cli.journal.facet.merge.scratch.failed = die Arbeitskopie { $path } des Probelaufs konnte nicht entfernt werden: { $error }; sonst wurde nichts geändert, und dieser Ordner kann gelöscht werden

journal.cli.journal.facet.merge.source.absent = '{ $source }' ist keine Facette

journal.cli.journal.facet.merge.source.declared = '{ $source }' ist eine deklarierte Facette, kein verwaister Ordner

journal.cli.journal.facet.merge.source.only = '{ $source }' ist die einzige Facette, die nicht stummgeschaltet ist, und ein Journal behält mindestens eine; hebe zuerst die Stummschaltung von '{ $destination }' auf

journal.cli.journal.facet.merge.source.retired = '{ $source }' ist ein stillgelegter Facettenname

journal.cli.journal.facet.merge.trash.failed = die Zusammenführung ist fertig, aber die Dateien, die sie ersetzt hat, konnten nicht in den Papierkorb verschoben werden: { $error }; sie werden behalten, und '{ $repair_command }' verschiebt sie dorthin

journal.cli.journal.facet.name.encoding = Facettenname ist nicht { $encoding }

journal.cli.journal.facet.name.unsafe = unsichere Facettendeklaration für { $slug }

journal.cli.journal.facet.notify.queue = die Suchaktualisierung konnte nicht in die Warteschlange gestellt werden: { $error }

journal.cli.journal.facet.notify.search = die Suche wurde nicht aktualisiert: { $reason }

journal.cli.journal.facet.path.encoding = { $path } ist nicht { $encoding }

journal.cli.journal.facet.path.plain = { $path } ist kein einfacher Pfad

journal.cli.journal.facet.record.declaration.damaged = Facettendeklaration ist beschädigt: { $detail }

journal.cli.journal.facet.record.declaration.exists = Facettendeklaration ist bereits vorhanden: { $path }

journal.cli.journal.facet.record.declaration.invalid = Facettendeklaration ist kein Objekt: { $path }

journal.cli.journal.facet.record.declaration.missing = Facettendeklaration fehlt: { $path }

journal.cli.journal.facet.record.declaration.unreadable = Facettendeklaration konnte nicht gelesen werden: { $detail }

journal.cli.journal.facet.record.id.duplicate = doppelter wohlgeformter Facettenbezeichner bereits auf dem Datenträger: { $id }

journal.cli.journal.facet.record.id.entropy = Zufallsbytes für den Bezeichner konnten nicht beschafft werden: { $error }

journal.cli.journal.facet.record.id.exhausted = erschöpfte Versuche, einen eindeutigen Facettenbezeichner zu erzeugen

journal.cli.journal.facet.record.link.invalid = Entitätsverknüpfung der Facette ist kein Objekt: { $path }

journal.cli.journal.facet.record.malformed.activity = Aktivitätsdefinitionen unter { $path }, Zeile { $line }, können nicht gelesen werden: { $reason }

journal.cli.journal.facet.record.malformed.observation = fehlerhafte Beobachtung { $source }, Zeile { $line }: { $reason }

journal.cli.journal.facet.record.marker.invalid = Abschlussmarkierung der Reparatur der Entitätsverknüpfung der Facette ist leer oder fehlerhaft: { $path }

journal.cli.journal.facet.record.minimum = Facette '{ $facet }' ist die einzige aktivierte Facette; ein Journal behält mindestens eine

journal.cli.journal.facet.record.missing.names = Objekt { $names_field } fehlt

journal.cli.journal.facet.record.name.retired = der Facettenname '{ $name }' gehörte zu einer Facette, die zusammengeführt oder umbenannt wurde, und führt jetzt zu ihr

journal.cli.journal.facet.record.not.object = kein { $json_format }-Objekt

journal.cli.journal.facet.record.reason.content = ein Objekt mit nichtleerem Zeichenketteninhalt erwartet

journal.cli.journal.facet.record.reason.history = ungültiges Feld { $history_field }

journal.cli.journal.facet.record.reason.id = { $id_field } muss eine vorzeichenlose Ganzzahl sein

journal.cli.journal.facet.record.reason.json = { $json_format } ist ungültig

journal.cli.journal.facet.record.reason.object = ein Objekt erwartet

journal.cli.journal.facet.record.reason.relation = ein Objekt mit nichtleerem Zeichenketteninhalt oder einer Beziehung erwartet

journal.cli.journal.facet.record.reason.retired = ungültiges Feld { $retired_field }

journal.cli.journal.facet.record.retired.conflict = der Facettenname '{ $name }' ist bereits als stillgelegt mit einem anderen Ergebnis aufgezeichnet

journal.cli.journal.facet.record.retired.damaged = { $retired_path } konnte nicht gelesen werden ({ $detail }); führe '{ $repair_command }' aus

journal.cli.journal.facet.record.source.snapshot = erfasstes Abbild

journal.cli.journal.facet.retire.argument = { $retire_flag } braucht { $name_argument } und danach { $into_facet } oder { $deleted_flag }

journal.cli.journal.facet.retire.deleted = '{ $name }' wurde als gelöscht aufgezeichnet. die Suche holt auf.

journal.cli.journal.facet.retire.encoding = Namen müssen { $encoding } sein

journal.cli.journal.facet.retire.folder = '{ $name }' ist ein Ordner in diesem Journal; nur ein Name, den es nicht mehr gibt, kann stillgelegt werden

journal.cli.journal.facet.retire.merged = '{ $name }' wurde als in '{ $target }' zusammengeführt aufgezeichnet. die Suche holt auf.

journal.cli.journal.facet.retire.name = '{ $name }' ist kein Facettenname

journal.cli.journal.facet.retire.target = '{ $target }' ist keine Facette mit einer stabilen ID; führe zuerst '{ $backfill_command }' aus

journal.cli.journal.facet.transaction.exists = Transaktionspfad ist bereits vorhanden: { $path }

journal.cli.journal.facet.transaction.rollback = Transaktion fehlgeschlagen und zurückgerollt: { $primary }

journal.cli.journal.facet.transaction.rollback.failed = Transaktion fehlgeschlagen ({ $primary }); das Zurückrollen ist ebenfalls fehlgeschlagen ({ $rollback_error })

journal.cli.journal.facet.trash.absent = das ist nicht mehr im Papierkorb

journal.cli.journal.facet.trash.busy = das wird bereits wiederhergestellt oder endgültig gelöscht

journal.cli.journal.facet.trash.io = dein Papierkorb konnte nicht geändert werden: { $error }

journal.cli.journal.facet.trash.missing = { $item } gibt es so in deinem Journal nicht

journal.cli.journal.facet.trash.unfinished = es liegt im Papierkorb, der es noch fertig weglegt

journal.cli.journal.facet.trash.unknown = dieser Build kann das nicht wiederherstellen

journal.cli.journal.help.overview = { $invocation } - das Journal auf diesem Computer{ $line_break }{ $line_break }Verwendung: { $syntax }{ $line_break }{ $line_break }{ $old_command } ist zu '{ $invocation }' umgezogen; nutze diesen Befehl für lokale Journal-Vorgänge.{ $line_break }{ $line_break }Lokale Befehle:{ $line_break }{ $commands }{ $line_break }{ $line_break }Prozessbefehle:{ $line_break }{ $processes }{ $line_break }{ $line_break }Journal-lokale Befehle:{ $line_break }{ $local }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_4 }Diese Hilfe anzeigen{ $line_break }{ $syntax_continuation_17 }{ $line_break }{ $syntax_continuation_18 }

journal.cli.journal.help.path = Verwendung: { $syntax }{ $line_break }{ $line_break }Gibt den aufgelösten Journal-Pfad aus.

journal.cli.journal.help.root = Verwendung: { $syntax }{ $line_break }{ $line_break }Gibt die Projektwurzel aus.

journal.cli.journal.help.setup.copy = Verwendung: { $syntax }

journal.cli.journal.help.status = Verwendung: { $syntax }{ $line_break }{ $line_break }Gibt aus, ob das Journal vorhanden ist und wie viele Tage es hat.

journal.cli.journal.help.usage = Verwendung: { $syntax }

journal.cli.journal.helper.inspect = { $code }: die native Programmdatei des Journals konnte nicht untersucht werden: { $error }

journal.cli.journal.helper.missing.mac = { $code }: { $path }. Installiere die Journal-App neu.

journal.cli.journal.helper.missing.other = { $code }: { $path }. Installiere { $package } neu.

journal.cli.journal.helper.mode.mac = { $code }: { $path }. Installiere die Journal-App neu.

journal.cli.journal.helper.mode.other = { $code }: { $path }. Installiere { $package } neu.

journal.cli.journal.host.path.utf8 = native Auflösung des Journal-Pfads fehlgeschlagen: der aufgelöste Journal-Pfad ist kein gültiges UTF-8

journal.cli.journal.host.status.absent = Journal: { $path }{ $line_break }Quelle: { $source }{ $line_break }Vorhanden: nein{ $line_break }Tage: { $days }

journal.cli.journal.host.status.directory = nativer Journal-Status fehlgeschlagen: die Journal-Wurzel ist kein Verzeichnis: { $path }

journal.cli.journal.host.status.exists = Journal: { $path }{ $line_break }Quelle: { $source }{ $line_break }Vorhanden: ja{ $line_break }Tage: { $days }

journal.cli.journal.host.status.inspect = nativer Journal-Status fehlgeschlagen: { $path } konnte nicht untersucht werden: { $error }

journal.cli.journal.host.status.utf8 = nativer Journal-Status fehlgeschlagen: der aufgelöste Journal-Pfad ist kein gültiges UTF-8

journal.cli.journal.indexer.argument.argument = unerwartetes Argument

journal.cli.journal.indexer.argument.budget = { $budget_seconds_flag } braucht eine positive Zahl

journal.cli.journal.indexer.argument.drain = { $drain_flag } erfordert { $apply_flag }

journal.cli.journal.indexer.argument.duplicate = unerwartetes oder doppeltes Argument

journal.cli.journal.indexer.argument.file = { $rescan_file_flag } erfordert { $path }

journal.cli.journal.indexer.argument.ios = nicht verfügbar auf { $ios }

journal.cli.journal.indexer.argument.missing.agent = { $agent_flag } erfordert { $agent }

journal.cli.journal.indexer.argument.missing.day = { $day_flag } erfordert { $day }

journal.cli.journal.indexer.argument.missing.day.from = { $day_from_flag } erfordert { $day }

journal.cli.journal.indexer.argument.missing.day.to = { $day_to_flag } erfordert { $day }

journal.cli.journal.indexer.argument.missing.facet = { $facet_flag } erfordert { $facet }

journal.cli.journal.indexer.argument.missing.limit = { $limit_flag } erfordert eine nichtnegative Ganzzahl

journal.cli.journal.indexer.argument.missing.offset = { $offset_flag } erfordert eine nichtnegative Ganzzahl

journal.cli.journal.indexer.argument.missing.stream = { $stream_flag } erfordert { $stream }

journal.cli.journal.indexer.argument.missing.top = { $top_flag } erfordert eine nichtnegative Ganzzahl

journal.cli.journal.indexer.argument.rescan.conflict = { $rescan_file_flag } kann nicht mit { $rescan_flag } oder { $rescan_full_flag } kombiniert werden

journal.cli.journal.indexer.argument.unsupported = nicht unterstützte Datei

journal.cli.journal.indexer.build.building = Build: wird erstellt

journal.cli.journal.indexer.build.complete = Build: abgeschlossen

journal.cli.journal.indexer.build.unknown = Build: unbekannt

journal.cli.journal.indexer.calendar.at = Zeile { $line }: { $reason }

journal.cli.journal.indexer.calendar.import = Kalendernotizen in { $rel } übersprungen: { $error }

journal.cli.journal.indexer.calendar.line.repeat = Kalendernotiz-Zeile { $line } wiederholt ein Ereignis

journal.cli.journal.indexer.calendar.line.skip = Kalendernotiz-Zeile { $line } übersprungen: { $reason }

journal.cli.journal.indexer.calendar.segment = Kalendernotizen übersprungen: { $rel } liegt in keinem Segment

journal.cli.journal.indexer.calendar.settings = Kalenderereignis in { $path } wird aufgelistet, sobald die Einstellungsdatei gelesen werden kann: { $error }

journal.cli.journal.indexer.calendar.source = Kalendernotizen werden nur aus Kalender-Streams und Kalenderimporten gelesen: { $rel }

journal.cli.journal.indexer.calendar.unlisted = Kalendernotiz in { $path } nicht aufgelistet: { $reason }

journal.cli.journal.indexer.calendar.validation.account.must.be.a.string.or.null = { $account_field } muss eine Zeichenkette oder { $null_key } sein

journal.cli.journal.indexer.calendar.validation.all.day.must.be.true.or.false = { $all_day_field } muss { $true_key } oder { $false_key } sein

journal.cli.journal.indexer.calendar.validation.attendee.status.must.be.a.string.or.null = Teilnehmer { $status_field } muss eine Zeichenkette oder { $null_key } sein

journal.cli.journal.indexer.calendar.validation.attendee.you.must.be.true.or.false = Teilnehmer { $you_field } muss { $true_key } oder { $false_key } sein

journal.cli.journal.indexer.calendar.validation.attendees.must.be.an.array = { $attendees_field } muss ein Array sein

journal.cli.journal.indexer.calendar.validation.change.must.be.added.changed.or.removed = { $change_field } muss { $added_changed_or_removed_key } sein

journal.cli.journal.indexer.calendar.validation.changed.at.falls.outside.the.segment.window = { $changed_at_field } fällt außerhalb des Segmentfensters

journal.cli.journal.indexer.calendar.validation.changed.at.must.be.a.time.with.an.offset = { $changed_at_field } muss eine Zeit mit einem Offset sein

journal.cli.journal.indexer.calendar.validation.each.attendee.must.be.an.object = jeder Teilnehmer muss ein Objekt sein

journal.cli.journal.indexer.calendar.validation.end.cannot.be.read.as.a.date.or.time = { $end_field } kann nicht als Datum oder Zeit gelesen werden

journal.cli.journal.indexer.calendar.validation.end.is.before.start = { $end_field } liegt vor dem Beginn

journal.cli.journal.indexer.calendar.validation.end.must.be.a.string.or.null = { $end_field } muss eine Zeichenkette oder { $null_key } sein

journal.cli.journal.indexer.calendar.validation.event.and.occurrence.repeat.an.earlier.line = { $event_field } und Vorkommen wiederholen eine frühere Zeile

journal.cli.journal.indexer.calendar.validation.event.must.be.a.non.empty.string = { $event_field } muss eine nicht leere Zeichenkette sein

journal.cli.journal.indexer.calendar.validation.expanded.must.be.true.when.present = { $expanded_field } muss { $true_key } sein, wenn vorhanden

journal.cli.journal.indexer.calendar.validation.field.missing = { $key } fehlt

journal.cli.journal.indexer.calendar.validation.field.type = { $key } muss eine Zeichenkette oder { $null_input } sein

journal.cli.journal.indexer.calendar.validation.file.encoding = Datei ist nicht { $encoding }

journal.cli.journal.indexer.calendar.validation.file.holds.no.notes = Datei hält keine Notizen

journal.cli.journal.indexer.calendar.validation.file.lines = Datei hält mehr als 5.000 Notizen

journal.cli.journal.indexer.calendar.validation.file.size = Datei hält mehr als 8 { $unit }

journal.cli.journal.indexer.calendar.validation.filename = Dateiname muss { $prefix } gefolgt von 64 kleingeschriebenen Hex-Ziffern und { $suffix } sein

journal.cli.journal.indexer.calendar.validation.import.must.be.a.non.empty.string = { $import_field } muss eine nicht leere Zeichenkette sein

journal.cli.journal.indexer.calendar.validation.imported.at.must.be.a.time.with.an.offset = { $imported_at_key } muss eine Zeit mit einem Offset sein

journal.cli.journal.indexer.calendar.validation.line.is.empty = Zeile ist leer

journal.cli.journal.indexer.calendar.validation.line.is.not.a.json.object = Zeile ist kein { $json_key }-Objekt

journal.cli.journal.indexer.calendar.validation.occurrence.cannot.be.read.as.a.date.or.time = { $occurrence_field } kann nicht als Datum oder Zeit gelesen werden

journal.cli.journal.indexer.calendar.validation.occurrence.must.be.a.string.or.null = { $occurrence_field } muss eine Zeichenkette oder { $null_key } sein

journal.cli.journal.indexer.calendar.validation.organizer.must.be.an.object.or.null = { $organizer_field } muss ein Objekt oder { $null_key } sein

journal.cli.journal.indexer.calendar.validation.person.field = { $what } { $key } muss eine Zeichenkette oder { $null_input } sein

journal.cli.journal.indexer.calendar.validation.segment.grid = Segment muss ein 300-Sekunden-Fenster auf dem Raster des Journals benennen

journal.cli.journal.indexer.calendar.validation.segment.must.name.a.span.on.its.day = Segment muss eine Spanne an seinem Tag benennen

journal.cli.journal.indexer.calendar.validation.start.and.end.must.be.dates.exactly.when.all.day.is.true = { $start_field } und { $end_field } müssen genau dann Datumsangaben sein, wenn { $all_day_field } { $true_key } ist

journal.cli.journal.indexer.calendar.validation.start.and.end.must.both.carry.an.offset.or.both.have.none = { $start_field } und { $end_field } müssen beide einen Offset tragen oder beide keinen haben

journal.cli.journal.indexer.calendar.validation.start.cannot.be.read.as.a.date.or.time = { $start_field } kann nicht als Datum oder Zeit gelesen werden

journal.cli.journal.indexer.calendar.validation.start.is.missing = { $start_field } fehlt

journal.cli.journal.indexer.calendar.validation.version = { $v_field } muss 1 sein

journal.cli.journal.indexer.chunker.activity = nicht unterstützte Desktop-Aktivitätsversion

journal.cli.journal.indexer.chunker.calendar = Kalendernotiz übersprungen: { $reason }

journal.cli.journal.indexer.chunker.lines = { $collapsed ->
    [one] Leerraum auf { $collapsed } überlanger Markdown-Zeile beim Indexieren zusammengefasst
   *[other] Leerraum auf { $collapsed } überlangen Markdown-Zeilen beim Indexieren zusammengefasst
}

journal.cli.journal.indexer.classification.absent = Klassifikationen: Datenbank nicht vorhanden

journal.cli.journal.indexer.classification.complete = Klassifikationen: abgeschlossen (verarbeitet { $processed }, nicht klassifiziert { $unclassified })

journal.cli.journal.indexer.classification.incomplete = Klassifikationen: unvollständiger Sidecar bei Cursor '{ $cursor }' (führe '{ $repair }' aus)

journal.cli.journal.indexer.classification.mismatch = Klassifikationen: Abweichung der Abdeckung bei Cursor '{ $cursor }' (verarbeitet { $processed }, verbleibend { $remaining }, fehlend { $missing }, nicht klassifiziert { $unclassified })

journal.cli.journal.indexer.classification.missing = { $missing ->
    [one] Abweichung der Abdeckung der Block-Klassifikation: { $missing } fehlende Zeile
   *[other] Abweichung der Abdeckung der Block-Klassifikation: { $missing } fehlende Zeilen
}

journal.cli.journal.indexer.classification.progress = Klassifikationen: in Bearbeitung bei Cursor '{ $cursor }' (verarbeitet { $processed }, verbleibend { $remaining }, fehlend { $missing }, nicht klassifiziert { $unclassified })

journal.cli.journal.indexer.classification.stall = Nachziehen der Block-Klassifikation ins Stocken geraten: { $err }

journal.cli.journal.indexer.classification.stalled = Klassifikationen: ins Stocken geraten bei '{ $path }' (verarbeitet { $processed }, verbleibend { $remaining }, fehlend { $missing }, nicht klassifiziert { $unclassified })

journal.cli.journal.indexer.classification.unready = Klassifikationen: nicht bereit bei Cursor '{ $cursor }' (führe '{ $repair_command }' aus)

journal.cli.journal.indexer.database.absent = Index: nicht vorhanden

journal.cli.journal.indexer.database.files = Dateien: { $discovered } auf dem Datenträger, { $indexed } indexiert, { $current } aktuell, { $stale } veraltet, { $missing } fehlend, { $failed } fehlgeschlagen, { $orphaned } verwaist, { $retained } behalten bis zu einem vollständigen Rescan, { $ineligible_rows } nicht mehr indexierbar, aber noch in der Suche, { $unreadable } nicht lesbar, { $ineligible } nicht indexierbar

journal.cli.journal.indexer.database.present = Index: vorhanden

journal.cli.journal.indexer.database.sqlite = keine { $sqlite }-Datenbank

journal.cli.journal.indexer.database.unreadable = Index: nicht lesbar ({ $reason })

journal.cli.journal.indexer.discovery.listing = Glob-Durchlauf fehlgeschlagen: der Versuch, `{ $path }` zu lesen, führte zu einem Fehler: { $error }

journal.cli.journal.indexer.discovery.pattern = ungültiges Glob-Muster: { $error }

journal.cli.journal.indexer.discovery.prefix = entdeckter Pfad { $path } liegt nicht unter der Wurzel { $root }

journal.cli.journal.indexer.discovery.relative = entdeckter Pfad ist kein gültiges { $encoding }: { $path }

journal.cli.journal.indexer.discovery.root = Glob-Wurzel ist kein gültiges { $encoding }: { $path }

journal.cli.journal.indexer.drain.result = { $processed ->
    [one] { $processed } Suchaktualisierung angewendet; { $failed } fehlgeschlagen; { $remaining } { $remaining ->
        [one] bleibt in der Warteschlange.
       *[other] bleiben in der Warteschlange.
    }
   *[other] { $processed } Suchaktualisierungen angewendet; { $failed } fehlgeschlagen; { $remaining } { $remaining ->
        [one] bleibt in der Warteschlange.
       *[other] bleiben in der Warteschlange.
    }
}

journal.cli.journal.indexer.edge.blob = Kantenzeile enthält einen nicht unterstützten Blob-Wert

journal.cli.journal.indexer.edge.day = Ungültiger Kanten-Tag: { $day }

journal.cli.journal.indexer.edge.dst = Kantenzeile erfordert eine nicht leere Zeichenkette { $dst_field }

journal.cli.journal.indexer.edge.event = Ereignis-Zeitstempel liegt außerhalb des { $integer }-Bereichs

journal.cli.journal.indexer.edge.integer = Ganzzahlfeld der Kante { $field } liegt außerhalb des { $integer }-Bereichs

journal.cli.journal.indexer.edge.json = Parsen der Kantenquelle { $json } fehlgeschlagen für { $path }: { $error }

journal.cli.journal.indexer.edge.kind = Unbekannte Kantenart: { $kind }

journal.cli.journal.indexer.edge.path = ungültiger Beobachtungspfad: { $path }

journal.cli.journal.indexer.edge.payload = { $source }-Payload muss ein { $json }-Objekt sein, stattdessen { $value_type }

journal.cli.journal.indexer.edge.refresh = Kanten-Auffrischung für { $rel } wird übersprungen: { $error }

journal.cli.journal.indexer.edge.relation = der Beobachtungsbeziehung fehlt { $kind_field }

journal.cli.journal.indexer.edge.segment.day = Ungültiger Segment-Tag: { $day }

journal.cli.journal.indexer.edge.segment.key = Ungültiger Segment-Schlüssel: { $segment }

journal.cli.journal.indexer.edge.segment.time = Segment-Zeitstempel liegt außerhalb des { $integer }-Bereichs

journal.cli.journal.indexer.edge.src = Kantenzeile erfordert eine nicht leere Zeichenkette { $src_field }

journal.cli.journal.indexer.edge.string = kann { $python } für den { $field }-Wert vom Typ { $value_type } nicht reproduzieren

journal.cli.journal.indexer.edge.target = Beobachtungsbeziehung { $target_field } muss eine nicht leere Zeichenkette sein

journal.cli.journal.indexer.edge.type = Kantenart für { $source } muss eine Zeichenkette sein, stattdessen { $value_type }

journal.cli.journal.indexer.edge.unsupported = Kantenfeld { $field } unterstützt { $value_type } nicht

journal.cli.journal.indexer.error.file = rescan-file fehlgeschlagen: { $detail }

journal.cli.journal.indexer.error.rebuild = Kanten-Neuaufbau fehlgeschlagen: { $detail }

journal.cli.journal.indexer.error.reset = Zurücksetzen fehlgeschlagen: { $detail }

journal.cli.journal.indexer.error.scan = Scan fehlgeschlagen: { $detail }

journal.cli.journal.indexer.files.activity.membership.insert = Einfügen der Aktivitätsmitgliedschaft fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.activity.provenance.insert = Einfügen der Aktivitätsherkunft fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.listing = Kalenderauflistung fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.note.insert = Einfügen der Kalendernotiz fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.group.offset.insert = Einfügen des Blockgruppen-Offsets fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.insert = Einfügen des Blocks fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.source.mapping = Zuordnung der Blockquelle fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.classification.insert = Einfügen der Klassifikation fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.content.read = Lesen des Inhalts fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.facet = Scan der Facettendeklaration fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.hidden = { $rel } ist vorerst aus der Suche ausgelassen: { $cause }

journal.cli.journal.indexer.files.memory.chain.serialization = Serialisierung der Erinnerungskette fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.memory.missing = validiertes Erinnerungsoriginal fehlt für { $rel }

journal.cli.journal.indexer.files.memory.origin.serialization = Serialisierung des Erinnerungsursprungs fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.memory.original.insert = Einfügen des Erinnerungsoriginals fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.files.memory.utf = Erinnerungsoriginal ist nicht { $utf } für { $rel }: { $error }

journal.cli.journal.indexer.files.partial = { $rel } ist vorerst teilweise aus der Suche ausgelassen: { $cause }

journal.cli.journal.indexer.files.screen.text.source.insert = Einfügen der Bildschirmtext-Quelle fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.generation.locked = Generation: { $generation } (diese Version liest bis { $known }; beschreibbar: nein)

journal.cli.journal.indexer.generation.writable = Generation: { $generation } (diese Version liest bis { $known }; beschreibbar: ja)

journal.cli.journal.indexer.help.drain = Verwendung: { $syntax }{ $line_break }wendet Suchaktualisierungen in der Warteschlange an. { $force_flag } bezieht verzögerte Wiederholungen ein.{ $line_break }{ $budget_seconds_flag } stoppt die Abarbeitung nach N Sekunden. eine Aktualisierung des ganzen Journals startet nicht, wenn weniger als 300 Sekunden übrig sind, und bleibt in der Warteschlange.

journal.cli.journal.indexer.help.main = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $syntax_continuation_4 }

journal.cli.journal.indexer.help.path = Verwendung: { $syntax }

journal.cli.journal.indexer.help.status = Verwendung: { $syntax }

journal.cli.journal.indexer.hidden.activity = lässt sich nicht erkennen, ob diese Aktivität verborgen ist: { $rel } ist nicht lesbar: { $error }

journal.cli.journal.indexer.hidden.damaged = lässt sich nicht erkennen, ob diese Aktivität verborgen ist: { $path } hat eine Zeile, die kein Datensatz ist

journal.cli.journal.indexer.hidden.days = lässt sich nicht erkennen, welche Tage der Körperdaten verborgen sind: { $path } ist nicht lesbar

journal.cli.journal.indexer.hidden.trash = lässt sich nicht erkennen, was das Journal erneut erstellt: { $error }

journal.cli.journal.indexer.hidden.week = lässt sich nicht erkennen, was auf dieser Wochenseite ausgelassen ist: { $path } ist nicht lesbar

journal.cli.journal.indexer.hidden.weeks = lässt sich nicht erkennen, was auf Wochenseiten ausgelassen ist: { $path } ist nicht lesbar

journal.cli.journal.indexer.merge.commit = Index ist korrekt und nur nicht verdichtet: { $fts }-Zusammenführung konnte nicht festgeschrieben werden: { $error }

journal.cli.journal.indexer.merge.failed = Index ist korrekt und nur nicht verdichtet: { $fts }-Zusammenführung fehlgeschlagen: { $error }

journal.cli.journal.indexer.merge.start = Index ist korrekt und nur nicht verdichtet: { $fts }-Zusammenführungstransaktion konnte nicht gestartet werden: { $error }

journal.cli.journal.indexer.migration.child = Segmentaggregat-Migration hat { $rel_segment } behalten: untergeordnete Datei ist nicht aktuell: { $rel }

journal.cli.journal.indexer.migration.discovery = Segmentaggregat-Migration hat { $rel_segment } behalten: Talent-Entdeckung fehlgeschlagen: { $error }

journal.cli.journal.indexer.migration.files = Indexdateien bleiben nach der Migration: { $paths }

journal.cli.journal.indexer.migration.mtime = Segmentaggregat-Migration hat { $rel_segment } behalten: Lesen von mtime fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.migration.retained = Segmentaggregat-Migration hat { $rel } behalten: { $error }

journal.cli.journal.indexer.migration.unclassified = Segmentaggregat-Migration hat { $rel_segment } behalten: nicht klassifiziertes Talent-Markdown { $rel }

journal.cli.journal.indexer.original.incomplete = Erinnerungsoriginal ist unvollständig für { $rel }: { $state }

journal.cli.journal.indexer.original.retained = Erinnerungsoriginal konnte für { $rel } nicht geprüft werden; zwischengespeicherter Index behalten: { $state }

journal.cli.journal.indexer.original.state.absent = nicht vorhanden

journal.cli.journal.indexer.original.state.corrupt = beschädigt

journal.cli.journal.indexer.original.state.deleted = gelöscht

journal.cli.journal.indexer.original.state.ready = bereit

journal.cli.journal.indexer.original.state.staged = vorgemerkt

journal.cli.journal.indexer.original.state.unavailable = nicht verfügbar ({ $detail })

journal.cli.journal.indexer.original.state.unready = nicht bereit

journal.cli.journal.indexer.path.ready = Pfad-Nachschlagen: bereit

journal.cli.journal.indexer.path.unready = Pfad-Nachschlagen: nicht bereit

journal.cli.journal.indexer.progress.absent = Klassifikation: nicht vorhanden

journal.cli.journal.indexer.progress.complete = Klassifikation: abgeschlossen

journal.cli.journal.indexer.progress.incomplete = Klassifikation: unvollständig

journal.cli.journal.indexer.progress.stalled = Klassifikation: ins Stocken geraten bei { $path }

journal.cli.journal.indexer.progress.unknown = Klassifikation: ins Stocken geraten bei unbekanntem Pfad

journal.cli.journal.indexer.query.column.agent = Agent

journal.cli.journal.indexer.query.column.day = Tag

journal.cli.journal.indexer.query.column.facet = Facette

journal.cli.journal.indexer.query.empty = Journal-Index ist leer: { $path }

journal.cli.journal.indexer.query.error = { $command }: { $detail }

journal.cli.journal.indexer.query.input = Lesen von stdin fehlgeschlagen: { $error }

journal.cli.journal.indexer.query.locked = Journal-Index ist gesperrt ({ $path }): { $detail }

journal.cli.journal.indexer.query.more = ... +{ $remaining } mehr

journal.cli.journal.indexer.query.none = Keine Ergebnisse gefunden

journal.cli.journal.indexer.query.output = Schreiben nach stdout fehlgeschlagen: { $error }

journal.cli.journal.indexer.query.owner = Verbindungsabfrage braucht dich

journal.cli.journal.indexer.query.prompt = Suche>

journal.cli.journal.indexer.query.showing = { $total ->
    [one] Zeige { $first }-{ $last } von { $total } Ergebnis
   *[other] Zeige { $first }-{ $last } von { $total } Ergebnissen
}

journal.cli.journal.indexer.query.unreadable = Journal-Index ist nicht lesbar ({ $path }): { $detail }

journal.cli.journal.indexer.receipt.absent = letzter Scan: nicht erfasst

journal.cli.journal.indexer.receipt.full.finished = letzter Scan: vollständig um { $time } ms, indexiert { $indexed }, entfernt { $removed }, fehlgeschlagen { $failed }

journal.cli.journal.indexer.receipt.full.stopped = letzter Scan: vollständig um { $time } ms angehalten: { $error }

journal.cli.journal.indexer.receipt.light.finished = letzter Scan: leicht um { $time } ms, indexiert { $indexed }, entfernt { $removed }, fehlgeschlagen { $failed }

journal.cli.journal.indexer.receipt.light.stopped = letzter Scan: leicht um { $time } ms angehalten: { $error }

journal.cli.journal.indexer.rel.absolute = { $rel } muss journal-{ $rel }ativ sein

journal.cli.journal.indexer.rel.backslash = { $rel } muss { $posix }-Trennzeichen verwenden

journal.cli.journal.indexer.rel.empty = { $rel } darf nicht leer sein

journal.cli.journal.indexer.rel.invalidcomponent = { $rel } darf keine leeren Komponenten, { $current_component } oder { $parent_component } enthalten

journal.cli.journal.indexer.rel.trash = { $rel } darf nicht den Papierkorb benennen

journal.cli.journal.indexer.scan.calendar.invites = Kalendereinladungen noch nicht verknüpft: { $error }

journal.cli.journal.indexer.scan.connections = In diesem Rescan kamen keine neuen Verbindungen dazu. Führe { $rebuild_command } aus, um Wartendes jetzt auf den neuesten Stand zu bringen.

journal.cli.journal.indexer.scan.connections.failed = Verbindungen nicht gegen Änderungen der Entitäten geprüft: { $error }

journal.cli.journal.indexer.scan.deferred = ein vollständiger Suchdurchgang arbeitet diese Änderungen bereits ab, daher hat diese Prüfung sie ihm überlassen. führe { $status_command } für Details aus.

journal.cli.journal.indexer.scan.edge.skip = Kantenextraktion für { $rel } wird übersprungen: { $error }

journal.cli.journal.indexer.scan.failed = { $indexed ->
    [one] Indexiert { $indexed } Datei, entfernt { $removed }, übersprungen { $skipped }, fehlgeschlagen { $failed }
   *[other] Indexiert { $indexed } Dateien, entfernt { $removed }, übersprungen { $skipped }, fehlgeschlagen { $failed }
}

journal.cli.journal.indexer.scan.failure = { $failed ->
    [one] { $command }: { $failed } Aktualisierung konnte nicht indexiert werden; sieh die Warnungen oben
   *[other] { $command }: { $failed } Aktualisierungen konnten nicht indexiert werden; sieh die Warnungen oben
}

journal.cli.journal.indexer.scan.invalid.segment = Kantenextraktion für { $rel } wird übersprungen: ungültiger Segment-Schlüssel { $segment }

journal.cli.journal.indexer.scan.mtime = Lesen von mtime fehlgeschlagen für { $rel }: { $error }

journal.cli.journal.indexer.scan.retained = { $count ->
    [one] leichter Scan hat { $count } veraltete Zeile für Tag { $day } behalten: die Entdeckung hat für diesen Tag keine Dateien erzeugt; führe ihn mit { $full_flag } erneut aus, um sie zu entfernen
   *[other] leichter Scan hat { $count } veraltete Zeilen für Tag { $day } behalten: die Entdeckung hat für diesen Tag keine Dateien erzeugt; führe ihn mit { $full_flag } erneut aus, um sie zu entfernen
}

journal.cli.journal.indexer.scan.summary = { $indexed ->
    [one] Indexiert { $indexed } Datei, entfernt { $removed }, übersprungen { $skipped }
   *[other] Indexiert { $indexed } Dateien, entfernt { $removed }, übersprungen { $skipped }
}

journal.cli.journal.indexer.speaker.duplicate = doppeltes Transkript der Sprecherkante { $field } { $sentence } für { $composite_id }

journal.cli.journal.indexer.speaker.missing = Transkript der Sprecherkante fehlt für { $composite_id }

journal.cli.journal.indexer.speaker.unresolved = Transkript der Sprecherkante unaufgelöst für { $composite_id }

journal.cli.journal.indexer.status.check = letzte Suchprüfung: um { $time } ms, indexiert { $indexed }, entfernt { $removed }

journal.cli.journal.indexer.status.check.absent = letzte Suchprüfung: nicht erfasst

journal.cli.journal.indexer.status.check.overdue = letzte Suchprüfung: überfällig, sie ist seitdem gelaufen, ohne fertig zu werden

journal.cli.journal.indexer.status.failure = Fehlschlag: { $reason }

journal.cli.journal.indexer.status.health = Zustand: { $state } ({ $description })

journal.cli.journal.indexer.status.queued = in der Warteschlange: { $queued }

journal.cli.journal.indexer.status.removals = Entfernungen warten noch: { $removals }

journal.cli.journal.indexer.status.wait.days = ältestes in der Warteschlange: wartet { $days } d { $hours } h

journal.cli.journal.indexer.status.wait.hours = ältestes in der Warteschlange: wartet { $hours } h { $minutes } min

journal.cli.journal.indexer.status.wait.minutes = ältestes in der Warteschlange: wartet { $minutes } min

journal.cli.journal.indexer.status.wait.seconds = ältestes in der Warteschlange: wartet { $seconds } s

journal.cli.journal.indexer.store.admission = Zulassung des Index-Schreibers fehlgeschlagen für { $operation }: { $cause }

journal.cli.journal.indexer.store.busy = Index-Schreiber beschäftigt für { $operation }: { $cause }

journal.cli.journal.indexer.store.encoding = Pfad ist kein gültiges { $encoding }: { $path }

journal.cli.journal.indexer.store.generation = der Suchindex wurde von einer neueren Version von { $brand } geschrieben (Index-Generation { $found }; diese Version liest bis { $known }), daher lässt diese Version ihn unverändert

journal.cli.journal.indexer.store.lookup.cause = { $lookup_command }: { $cause }

journal.cli.journal.indexer.store.missing = Datei nicht gefunden: { $path }

journal.cli.journal.indexer.store.original = Erinnerungsoriginal konnte unter { $path } nicht geprüft werden; zwischengespeicherter Index behalten; versuch es noch einmal, wenn die Quelle lesbar ist: { $detail }

journal.cli.journal.indexer.store.outside = Datei liegt außerhalb des Journal-Verzeichnisses: { $path }

journal.cli.journal.indexer.stream.json = Stream-Markierung { $syntax } fehlgeschlagen für { $path }: { $error }

journal.cli.journal.indexer.stream.path = Pfad der Stream-Markierung fehlgeschlagen für { $rel_segment }: { $error }

journal.cli.journal.indexer.stream.read = Lesen der Stream-Markierung fehlgeschlagen für { $path }: { $error }

journal.cli.journal.indexer.warning = Warnung: { $warning }

journal.cli.journal.local.directory = kein echtes Verzeichnis: { $path }

journal.cli.journal.local.failure = { $invocation } { $token }: { $message }

journal.cli.journal.local.resolution.config = Journal-Auflösung fehlgeschlagen: Dekodieren der Konfiguration fehlgeschlagen

journal.cli.journal.local.resolution.home = Journal-Auflösung fehlgeschlagen: Home-Verzeichnis nicht verfügbar

journal.cli.journal.local.unknown = unbekannter lokaler Vorgang

journal.cli.journal.notify.error.facet = { $invocation }: Fehler: { $option } wird nicht mehr unterstützt. Setze die Facettenauswahl in { $action_flag }; zum Beispiel { $action_flag } { $example }.

journal.cli.journal.notify.error.integer = { $invocation }: Fehler: Argument { $auto_dismiss_flag }: ungültiger Ganzzahlwert: '{ $value }'

journal.cli.journal.notify.error.message = { $invocation }: Fehler: die folgenden Argumente sind erforderlich: { $positional }

journal.cli.journal.notify.error.unknown = { $invocation }: Fehler: nicht erkannte Argumente: { $token }

journal.cli.journal.notify.error.value = { $invocation }: Fehler: Argument { $option }: ein Argument erwartet

journal.cli.journal.notify.failed = Die Benachrichtigung konnte nicht gesendet werden (läuft { $channel }?)

journal.cli.journal.notify.help = Verwendung: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                  { $message_options }{ $line_break }{ $line_break }Sendet eine Benachrichtigung über { $channel }{ $line_break }{ $line_break }Positionsargumente:{ $line_break }{ $positional }Text der Benachrichtigung{ $line_break }{ $line_break }Optionen:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $indent_2 }{ $title_option }{ $gap_9 }Titel der Benachrichtigung{ $line_break }{ $indent_2 }{ $icon_option }{ $gap_11 }{ $icon_set } Symbolname (Standard: { $mailbox }){ $line_break }{ $indent_2 }{ $event_option }{ $gap_9 }Ereignisname (Standard: { $default_show }{ $line_break }{ $indent_2 }{ $action_option }{ $gap_7 }URL-Pfad, der bei einem Klick geöffnet wird{ $line_break }{ $indent_2 }{ $app_option }{ $gap_13 }Name der Quell-App{ $line_break }{ $indent_2 }{ $badge_option }{ $gap_9 }Badge-Text oder Zahl{ $line_break }{ $syntax_continuation_18 }{ $line_break }                        automatisch ausblenden nach N Millisekunden{ $line_break }{ $indent_2 }{ $no_dismiss_option }{ $gap_10 }Benachrichtigung nicht ausblendbar machen{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Ausführliche Ausgabe aktivieren{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Debug-Protokollierung aktivieren

journal.cli.journal.notify.sent = Benachrichtigung gesendet

journal.cli.journal.notify.utf8 = native Journal-Benachrichtigung fehlgeschlagen: deine Argumente sind kein gültiges UTF-8

journal.cli.journal.process.admission = Zulassung des Journal-Prozesses fehlgeschlagen: { $detail }

journal.cli.journal.process.body = nativer Start des Journal-Prozesses fehlgeschlagen: keine native Implementierung für { $token }

journal.cli.journal.process.launch = nativer Start des Journal-Prozesses fehlgeschlagen: { $error }

journal.cli.journal.resolution.config = native Journal-Auflösung fehlgeschlagen: Dekodieren der Konfiguration fehlgeschlagen

journal.cli.journal.resolution.home = native Journal-Auflösung fehlgeschlagen: Home-Verzeichnis nicht verfügbar

journal.cli.journal.root.inspect = native Auflösung der Journal-Projektwurzel fehlgeschlagen: die aktuelle Programmdatei konnte nicht untersucht werden: { $error }

journal.cli.journal.root.package = native Auflösung der Journal-Projektwurzel fehlgeschlagen: Quell-Checkout oder installiertes { $product }-Paket konnte von { $path } aus nicht gefunden werden

journal.cli.sol.call.activities.day.invalid = Fehler: ungültiger Tag '{ $value }'

journal.cli.sol.call.activities.day.missing = Fehler: der Tag ist erforderlich (als Argument übergeben oder { $literal_1 } setzen).

journal.cli.sol.call.activities.empty = Keine Aktivitäten gefunden.

journal.cli.sol.call.activities.exists = Fehler: die Aktivität existiert bereits: { $detail }

journal.cli.sol.call.activities.facet.missing = Fehler: die Facette ist erforderlich (als Argument übergeben oder { $literal_1 } setzen).

journal.cli.sol.call.activities.facet.unknown = Fehler: unbekannte Aktivität für die Facette '{ $facet }': { $activity_type }

journal.cli.sol.call.activities.kind.option = Fehler: { $literal_1 } ist erforderlich.

journal.cli.sol.call.activities.kind.required = Fehler: die Aktivität ist erforderlich.

journal.cli.sol.call.activities.participation.array = Fehler: { $literal_1 } muss ein Array sein

journal.cli.sol.call.activities.participation.confidence = Fehler: { $literal_1 }[{ $index }] '{ $literal_2 }' muss eine Zahl sein

journal.cli.sol.call.activities.participation.context = Fehler: { $literal_1 }[{ $index }] '{ $literal_2 }' muss eine Zeichenkette sein

journal.cli.sol.call.activities.participation.name = Fehler: { $literal_1 }[{ $index }] erfordert eine nicht-leere Zeichenkette '{ $literal_2 }'

journal.cli.sol.call.activities.participation.object = Fehler: { $literal_1 }[{ $index }] muss ein Objekt sein

journal.cli.sol.call.activities.participation.role = Fehler: { $literal_1 }[{ $index }] hat die ungültige Rolle '{ $role }' (muss einer von ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }'] sein)

journal.cli.sol.call.activities.participation.source = Fehler: { $literal_1 }[{ $index }] hat die ungültige Quelle '{ $source }' (muss einer von ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }', '{ $literal_6 }'] sein)

journal.cli.sol.call.activities.patch.disallowed = Fehler: unzulässige Aktualisierungsfelder: { $fields }

journal.cli.sol.call.activities.patch.empty = Fehler: die Aktualisierungs-Payload muss mindestens ein veränderbares Feld enthalten.

journal.cli.sol.call.activities.range.options = Fehler: { $literal_1 } ist inkompatibel mit { $literal_2 }/{ $literal_3 }.

journal.cli.sol.call.activities.range.reversed = Fehler: { $literal_1 } ({ $end_day }) darf nicht vor { $literal_2 } ({ $start_day }) liegen

journal.cli.sol.call.activities.segment.invalid = Fehler: { $literal_1 } '{ $segment }' ist ungültig (erwartet { $literal_2 })

journal.cli.sol.call.activities.source.create = Fehler: { $literal_1 } muss '{ $literal_2 }' sein.

journal.cli.sol.call.activities.source.list = Fehler: { $literal_1 } muss '{ $literal_2 }' oder '{ $literal_3 }' sein.

journal.cli.sol.call.activities.span.absent = Aktivität nicht gefunden: { $span_id }

journal.cli.sol.call.activities.span.missing = Fehler: fehlendes Argument '{ $literal_1 }'.

journal.cli.sol.call.activities.stdin.json = Fehler: { $literal_1 } auf stdin ist ungültig: { $error }

journal.cli.sol.call.activities.stdin.object = Fehler: { $literal_1 }-Objekt auf stdin erwartet.

journal.cli.sol.call.activities.title.option = Fehler: { $literal_1 } ist erforderlich.

journal.cli.sol.call.activities.title.required = Fehler: der Titel ist erforderlich.

journal.cli.sol.call.body.coverage = Abdeckung: { $start } bis { $end }

journal.cli.sol.call.body.day = Tag: { $day }

journal.cli.sol.call.body.days = Tage: { $days }

journal.cli.sol.call.body.entries = Einträge: { $count }

journal.cli.sol.call.body.imports = Importe: { $imported }

journal.cli.sol.call.body.range = Zeitfenster: { $start } bis { $end }

journal.cli.sol.call.body.sources = Quellen: { $sources }

journal.cli.sol.call.body.unreadable = die Antwort der Körper-App konnte nicht gelesen werden.

journal.cli.sol.call.facets.accepted = Facettenkandidat '{ $name_key }' wurde als '{ $facet }' angenommen.

journal.cli.sol.call.facets.action = { $action }-Ergebnis für '{ $name_key }': { $result }

journal.cli.sol.call.facets.already.accepted = Facettenkandidat '{ $name_key }' ist bereits angenommen.

journal.cli.sol.call.facets.already.dismissed = Facettenkandidat '{ $name_key }' ist bereits verworfen.

journal.cli.sol.call.facets.candidate = { $name }  [{ $status }]  count={ $count }  last={ $day }

journal.cli.sol.call.facets.dismissed = Facettenkandidat '{ $name_key }' wurde verworfen.

journal.cli.sol.call.facets.empty = Keine Facettenkandidaten gefunden.

journal.cli.sol.call.thinking.attestation.expires = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.observed = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.reason = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.state = { $field_key }: { $value }

journal.cli.sol.call.thinking.continuing = die Operation läuft serverseitig weiter; { $literal_1 } zeigt ihren Fortschritt.

journal.cli.sol.call.thinking.enabled = { $field_key }: { $value }

journal.cli.sol.call.thinking.ended = die Operation endete, ohne die vertrauliche Verarbeitung zu aktivieren; prüfe { $literal_1 }.

journal.cli.sol.call.thinking.lane = aktive Lane: { $lane }

journal.cli.sol.call.thinking.next = weiter: { $literal_1 }

journal.cli.sol.call.thinking.not.ready = nicht bereit

journal.cli.sol.call.thinking.operation = Operation: { $operation_phase }

journal.cli.sol.call.thinking.portal = fortfahren im Browser { $arrow } { $url }

journal.cli.sol.call.thinking.provenance = { $field_key }: { $value }

journal.cli.sol.call.thinking.ready = bereit

journal.cli.sol.call.thinking.subscribe = { $field_key }: { $url }

journal.cli.sol.call.transcripts.actionable = { $marker } handlungsrelevant: { $literal_1 }

journal.cli.sol.call.transcripts.day.totals = { $day }  Transkripte:{ $n_transcripts } Perzepte:{ $n_percepts }

journal.cli.sol.call.transcripts.full.raw = Fehler: { $literal_1 } und { $literal_2 } können nicht zusammen verwendet werden.

journal.cli.sol.call.transcripts.heading.percepts = Perzepte:

journal.cli.sol.call.transcripts.heading.transcripts = Transkripte:

journal.cli.sol.call.transcripts.length.integer = Fehler: die Option { $literal_1 } erfordert eine ganze Zahl.

journal.cli.sol.call.transcripts.max.integer = Fehler: die Option { $literal_1 } erfordert eine ganze Zahl.

journal.cli.sol.call.transcripts.month.empty = Keine Daten für { $month }.

journal.cli.sol.call.transcripts.none = (keine)

journal.cli.sol.call.transcripts.pending = { $count ->
    [one] { $indent }{ $start } - { $end } ({ $display_count } Segment ausstehend bei { $starts })
   *[other] { $indent }{ $start } - { $end } ({ $display_count } Segmente ausstehend bei { $starts })
    }

journal.cli.sol.call.transcripts.range.flags = Fehler: { $literal_1 }, { $literal_2 } und { $literal_3 }/{ $literal_4 } dürfen nicht gemischt werden.

journal.cli.sol.call.transcripts.segments.empty = Keine Segmente.

journal.cli.sol.call.transcripts.source.flags = Fehler: { $literal_1 }/{ $literal_2 } darf nicht mit einzelnen Quellen-Flags gemischt werden.

journal.cli.sol.call.transcripts.speakers.heading = Sprecher für { $day }/{ $stream }/{ $segment }:

journal.cli.sol.call.transcripts.start.invalid = Fehler: die Startzeit muss { $literal_1 } sein.

journal.cli.sol.call.transcripts.tag.owner = { $marker } nicht handlungsrelevant: { $literal_1 }

journal.cli.sol.call.transcripts.total.days = { $count ->
    [one] Gesamt: { $display_count } Tag mit Daten
   *[other] Gesamt: { $display_count } Tage mit Daten
    }

journal.cli.sol.call.transcripts.truncated = [gekürzt: { $total } Bytes insgesamt, { $literal_1 } { $maximum }]

journal.cli.sol.call.transcripts.unknown.voice = unbekannte Stimme

journal.cli.sol.call.typer.error = Fehler

journal.cli.sol.call.typer.missing.argument = Fehlendes Argument '{ $name }'.

journal.cli.sol.call.typer.missing.option = Fehlende Option '{ $option }'.

journal.cli.sol.call.typer.try = Versuch '{ $command } { $literal_1 }' für Hilfe.

journal.cli.sol.entities.accept.merge.candidate.accept.result.for = Annahmeergebnis für { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.accept.merge.candidate.accepted.merge.candidate = Zusammenführungskandidat angenommen: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.candidate.already.accepted = Zusammenführungskandidat bereits angenommen: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.is.permanent = Zusammenführung { $merge_id } ist dauerhaft.

journal.cli.sol.entities.aka.added.alias.to = Alias '{ $aka_value }' zu '{ $resolved_name }' hinzugefügt.

journal.cli.sol.entities.aka.alias.already.exists.for = Alias '{ $aka_value }' existiert bereits für '{ $resolved_name }'.

journal.cli.sol.entities.aka.alias.is.the.first.word = Alias '{ $aka_value }' ist das erste Wort von '{ $resolved_name }' (übersprungen).

journal.cli.sol.entities.ambiguities.error.status.must.be.open = Fehler: { $option_status } muss open oder resolved sein.

journal.cli.sol.entities.ambiguities.from = { $spacing_2 }von: { $sources }

journal.cli.sol.entities.ambiguities.no.entity.ambiguities.found = Keine Entitätsmehrdeutigkeiten gefunden.

journal.cli.sol.entities.ambiguities.scope.tier = { $ambiguity_id }{ $spacing_2 }{ $name }{ $spacing_2 }[{ $status }]{ $spacing_2 }scope={ $scope }{ $spacing_2 }tier={ $tier }

journal.cli.sol.entities.ambiguities.score = { $spacing_2 }- { $candidate } score={ $score }

journal.cli.sol.entities.attach.entity.already.attached = Entität '{ $entity }' bereits zugeordnet.

journal.cli.sol.entities.attach.entity.attached = Entität '{ $entity }' zugeordnet.

journal.cli.sol.entities.busy = diese Entität konnte gerade nicht aktualisiert werden, weil sie beschäftigt war. versuch es gleich noch einmal.

journal.cli.sol.entities.delete.approval = Löschen verschiebt die Entität und ihre Facettenverknüpfungen in den Papierkorb; { $option_erase } löscht sie endgültig. Sobald du es genehmigst, führe es erneut mit { $option_yes } aus.

journal.cli.sol.entities.delete.entity.erased = Endgültig gelöscht: '{ $entity_id }'.

journal.cli.sol.entities.delete.entity.error.wasn.t.deleted = Fehler: '{ $entity_id }' wurde nicht gelöscht: { $reason }.

journal.cli.sol.entities.delete.failed.without.reason.error.wasn.t.deleted.it = Fehler: '{ $entity_id }' wurde nicht gelöscht: es wurde nicht gelöscht.

journal.cli.sol.entities.detect.entity.detected.for = Entität '{ $entity_name }' für { $day } erkannt.

journal.cli.sol.entities.dismiss.merge.candidate.dismiss.result.for = Verwerfungsergebnis für { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.dismiss.merge.candidate.dismissed.merge.candidate = Zusammenführungskandidat verworfen: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.dismiss.merge.candidate.merge.candidate.already.dismissed = Zusammenführungskandidat bereits verworfen: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found = Fehler: Entität '{ $query }' nicht gefunden.

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found.did = Fehler: Entität '{ $query }' nicht gefunden. Meintest du: { $labels }

journal.cli.sol.entities.edge.resolution.without.query.empty.error.entity.entity.not.found = Fehler: Entität 'entity' nicht gefunden.

journal.cli.sol.entities.edge.resolution.without.query.suggestions.error.entity.entity.not.found = Fehler: Entität 'entity' nicht gefunden. Meintest du: { $labels }

journal.cli.sol.entities.edges.unavailable = Verbindungen sind gerade nicht verfügbar.

journal.cli.sol.entities.entity.error.error.entity.is.hidden = Fehler: Entität '{ $name }' ist ausgeblendet.

journal.cli.sol.entities.entity.error.error.entity.not.found = Fehler: Entität '{ $name }' nicht gefunden.

journal.cli.sol.entities.entity.error.error.invalid.entity.type = Fehler: Ungültiger Entitätstyp '{ $entity_type }'.

journal.cli.sol.entities.entity.versions.merge = { $spacing_2 }merge={ $merge_id } ({ $description })

journal.cli.sol.entities.entity.versions.no.identity.history.for = Kein Identitätsverlauf für { $entity }.

journal.cli.sol.entities.entity.versions.unknown.no.identity.history.for.entity = Kein Identitätsverlauf für entity.

journal.cli.sol.entities.error.without.name.hidden.error.entity.entity.is.hidden = Fehler: Entität 'entity' ist ausgeblendet.

journal.cli.sol.entities.evidence.default.day.unknown.day = unbekannter-Tag

journal.cli.sol.entities.evidence.kind.missing.unknown = unbekannt

journal.cli.sol.entities.history.all.evidence.rows.for = { $quantity ->
    [one] { $total } Nachweiszeile für { $entity_id } <-> { $peer }:
   *[other] { $total } Nachweiszeilen für { $entity_id } <-> { $peer }:
    }

journal.cli.sol.entities.history.no.recorded.connection.history.between = Kein aufgezeichneter Verbindungsverlauf zwischen { $entity_id } und { $peer }.

journal.cli.sol.entities.history.no.visible.rows = { $quantity ->
    [one] { $total } Nachweiszeile für { $entity_id } <-> { $peer } (0 angezeigt):
   *[other] { $total } Nachweiszeilen für { $entity_id } <-> { $peer } (0 angezeigt):
    }

journal.cli.sol.entities.history.range.evidence.rows.for.showing = { $quantity ->
    [one] { $total } Nachweiszeile für { $entity_id } <-> { $peer } ({ $first }-{ $last } angezeigt):
   *[other] { $total } Nachweiszeilen für { $entity_id } <-> { $peer } ({ $first }-{ $last } angezeigt):
    }

journal.cli.sol.entities.list.attached.attached.entities = { $quantity ->
    [one] { $count } zugeordnete Entität:
   *[other] { $count } zugeordnete Entitäten:
    }

journal.cli.sol.entities.list.detected.detected.for.entities = { $quantity ->
    [one] { $count } Entität erkannt für { $day }:
   *[other] { $count } erkannt für { $day } Entitäten:
    }

journal.cli.sol.entities.list.hidden.hidden = { $spacing_2 }- { $name } ausgeblendet ({ $entity_type }): { $description }

journal.cli.sol.entities.list.no.entities.found = Keine Entitäten gefunden.

journal.cli.sol.entities.list.visible.row = { $spacing_2 }- { $name } ({ $entity_type }): { $description }

journal.cli.sol.entities.merge.approval = Eine Zusammenführung lässt sich nicht rückgängig machen. Sieh sie dir ohne { $option_commit } in der Vorschau an, dann, sobald du es genehmigst, führe sie erneut mit { $option_commit } { $option_yes } aus.

journal.cli.sol.entities.merge.candidates.facet.detections.needs.last = { $source } -> { $target }{ $spacing_2 }[{ $status }]{ $spacing_2 }facet={ $facet }{ $spacing_2 }detections={ $detections }{ $spacing_2 }needs={ $needs }{ $spacing_2 }last={ $last }

journal.cli.sol.entities.merge.candidates.no.merge.candidates.found = Keine Zusammenführungskandidaten gefunden.

journal.cli.sol.entities.merge.failure.default.error.operation.failed = Fehler: Vorgang fehlgeschlagen

journal.cli.sol.entities.merge.preview.aliases.added = { $spacing_2 }Aliase hinzugefügt: { $aliases }

journal.cli.sol.entities.merge.preview.emails.added = { $spacing_2 }E-Mails hinzugefügt: { $emails }

journal.cli.sol.entities.merge.preview.facet.links.moved.merged = { $spacing_2 }Facettenverknüpfungen: { $moved } verschoben, { $merged } zusammengeführt

journal.cli.sol.entities.merge.preview.merge.preview = Zusammenführungsvorschau:

journal.cli.sol.entities.merge.preview.observations.moved = { $spacing_2 }Beobachtungen verschoben: { $observations }

journal.cli.sol.entities.merge.preview.segment.update.errors = { $spacing_2 }Fehler der Segmentaktualisierung: { $errors }

journal.cli.sol.entities.merge.preview.speaker.labels.updated.labels.corrections = { $quantity ->
    [one] { $corrections_quantity ->
        [one] { $spacing_2 }Sprecherbezeichnungen aktualisiert: { $labels } Bezeichnung, { $corrections } Korrektur
       *[other] { $spacing_2 }Sprecherbezeichnungen aktualisiert: { $labels } Bezeichnung, { $corrections } Korrekturen
    }
   *[other] { $corrections_quantity ->
        [one] { $spacing_2 }Sprecherbezeichnungen aktualisiert: { $labels } Bezeichnungen, { $corrections } Korrektur
       *[other] { $spacing_2 }Sprecherbezeichnungen aktualisiert: { $labels } Bezeichnungen, { $corrections } Korrekturen
    }
    }

journal.cli.sol.entities.merge.preview.this.preview.covers.alias.and = { $spacing_2 }Diese Vorschau umfasst Alias- und E-Mail-Ergänzungen; die Zusammenführung kann auch Facettenverknüpfungen, Notizen, Sprecherbezeichnungen und Stimmproben ändern.

journal.cli.sol.entities.merge.preview.voice.samples.moved.added.total = { $spacing_2 }Stimmproben verschoben: { $added } hinzugefügt, { $total } insgesamt

journal.cli.sol.entities.move.entity.error.facet.from.does.not = Fehler: Facette '{ $from_facet }' ({ $option_from }) existiert nicht.

journal.cli.sol.entities.move.entity.error.facet.to.does.not = Fehler: Facette '{ $to_facet }' ({ $option_to }) existiert nicht.

journal.cli.sol.entities.move.entity.moved.entity.from.to = Entität '{ $entity_name }' von '{ $from_facet }' nach '{ $to_facet }' verschoben.

journal.cli.sol.entities.neighbor.count = count={ $count }

journal.cli.sol.entities.neighbor.kinds = kinds={ $kinds }

journal.cli.sol.entities.neighbor.score = score={ $score }

journal.cli.sol.entities.network.all.recorded.connections.for = { $quantity ->
    [one] { $total } aufgezeichnete Verbindung für { $entity_id }:
   *[other] { $total } aufgezeichnete Verbindungen für { $entity_id }:
    }

journal.cli.sol.entities.network.directed.both.directed.out.in = directed=out:{ $out_count }, in:{ $in_count }

journal.cli.sol.entities.network.directed.in.directed.in = directed=in:{ $in_count }

journal.cli.sol.entities.network.directed.out.directed.out = directed=out:{ $out_count }

journal.cli.sol.entities.network.limited.recorded.connections.for.showing = { $quantity ->
    [one] { $total } aufgezeichnete Verbindung für { $entity_id } ({ $shown } angezeigt):
   *[other] { $total } aufgezeichnete Verbindungen für { $entity_id } ({ $shown } angezeigt):
    }

journal.cli.sol.entities.network.no.recorded.connections.for = Keine aufgezeichneten Verbindungen für { $entity_id }.

journal.cli.sol.entities.network.seen.range.seen = seen={ $first }..{ $last }

journal.cli.sol.entities.network.seen.seen = seen={ $day }

journal.cli.sol.entities.observation.error.couldn.t.read.the.moments = die für '{ $name }' gespeicherten Momente konnten nicht gelesen werden: Zeile { $line_str } der Datei ist ungültig. nichts wurde geändert.

journal.cli.sol.entities.observations.newest.moments.for.in.newest.first = { $quantity ->
    [one] { $total } Moment für '{ $name }' in '{ $facet }', neueste zuerst:
   *[other] { $total } Momente für '{ $name }' in '{ $facet }', neueste zuerst:
    }

journal.cli.sol.entities.observations.nothing.with.yet = noch nichts mit { $resolved_name }.

journal.cli.sol.entities.observations.oldest.moments.for.in.oldest.first = { $quantity ->
    [one] { $total } Moment für '{ $name }' in '{ $facet }', älteste zuerst:
   *[other] { $total } Momente für '{ $name }' in '{ $facet }', älteste zuerst:
    }

journal.cli.sol.entities.observations.partial.most.recent.showing.the.most.recent.of = die { $shown } neuesten von { $total } Momenten für '{ $name }' werden angezeigt. füg { $option_all } hinzu, um jeden einzelnen zu sehen.

journal.cli.sol.entities.observations.partial.oldest.showing.the.oldest.of.moments = die { $shown } ältesten von { $total } Momenten für '{ $name }' werden angezeigt. füg { $option_all } hinzu, um jeden einzelnen zu sehen.

journal.cli.sol.entities.observe.added.added.moments.for.in = { $quantity ->
    [one] hinzugefügt. { $count } Moment für '{ $name }' in '{ $facet }'.
   *[other] hinzugefügt. { $count } Momente für '{ $name }' in '{ $facet }'.
    }

journal.cli.sol.entities.observe.your.journal.already.holds.that = dein Journal hält bereits diesen Moment für '{ $resolved_name }'. nichts wurde hinzugefügt.

journal.cli.sol.entities.overview.all.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
        [one] Netzwerküberblick: { $edges } Kante über { $entities } Entität hinweg:
       *[other] Netzwerküberblick: { $edges } Kante über { $entities } Entitäten hinweg:
    }
   *[other] { $entities_quantity ->
        [one] Netzwerküberblick: { $edges } Kanten über { $entities } Entität hinweg:
       *[other] Netzwerküberblick: { $edges } Kanten über { $entities } Entitäten hinweg:
    }
    }

journal.cli.sol.entities.overview.kinds = Arten: { $kinds }

journal.cli.sol.entities.overview.limited.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
        [one] Netzwerküberblick: { $edges } Kante über { $entities } Entität hinweg ({ $shown } angezeigt):
       *[other] Netzwerküberblick: { $edges } Kante über { $entities } Entitäten hinweg ({ $shown } angezeigt):
    }
   *[other] { $entities_quantity ->
        [one] Netzwerküberblick: { $edges } Kanten über { $entities } Entität hinweg ({ $shown } angezeigt):
       *[other] Netzwerküberblick: { $edges } Kanten über { $entities } Entitäten hinweg ({ $shown } angezeigt):
    }
    }

journal.cli.sol.entities.overview.no.recorded.connections.in.the = Keine aufgezeichneten Verbindungen im Kantenindex.

journal.cli.sol.entities.overview.type = type={ $entity_type }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.recorded = Zusammenführungskandidat aufgezeichnet: { $source } -> { $target }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.updated.status = Zusammenführungskandidat aktualisiert: { $source } -> { $target } (Status: { $status })

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.member = Fehler: erforderliche Option { $option_member_id } fehlt

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.revision = Fehler: erforderliche Option { $option_revision } fehlt

journal.cli.sol.entities.resolve.ambiguity.group.refusing.to.resolve.this.ambiguity = Ohne { $option_yes } wird diese Mehrdeutigkeitsgruppe nicht aufgelöst.

journal.cli.sol.entities.resolve.ambiguity.group.resolved.ambiguities.to = { $quantity ->
    [one] { $count } Mehrdeutigkeit zu { $entity_id } aufgelöst.
   *[other] { $count } Mehrdeutigkeiten zu { $entity_id } aufgelöst.
    }

journal.cli.sol.entities.resolve.ambiguity.refusing.to.resolve.this.ambiguity = Ohne { $option_yes } wird diese Mehrdeutigkeit nicht aufgelöst.

journal.cli.sol.entities.resolve.ambiguity.resolved.to.at = { $ambiguity_id } zu { $entity_id } aufgelöst am { $resolved_at }.

journal.cli.sol.entities.resolve.day.error.day.is.required.pass = Fehler: Tag ist erforderlich (als Argument übergeben oder { $literal_sol_day } setzen).

journal.cli.sol.entities.resolve.error.error.entity.is.hidden = Fehler: Entität '{ $blocked_name }' ist ausgeblendet.

journal.cli.sol.entities.resolve.error.error.entity.not.found.did = Fehler: Entität '{ $entity }' nicht gefunden. Meintest du: { $names }

journal.cli.sol.entities.resolve.error.error.entity.not.found.in = Fehler: Entität '{ $entity }' in Facette '{ $facet }' nicht gefunden.

journal.cli.sol.entities.resolve.facet.error.facet.is.required.pass = Fehler: Facette ist erforderlich (als Argument übergeben oder { $literal_sol_facet } setzen).

journal.cli.sol.entities.restore.review.error.missing.required.option.ambiguity = Fehler: erforderliche Option { $option_ambiguity_id } fehlt

journal.cli.sol.entities.restore.review.error.missing.required.option.facet = Fehler: erforderliche Option { $option_facet } fehlt

journal.cli.sol.entities.restore.review.error.missing.required.option.source = Fehler: erforderliche Option { $option_source_slug } fehlt

journal.cli.sol.entities.restore.review.error.missing.required.option.target = Fehler: erforderliche Option { $option_target_slug } fehlt

journal.cli.sol.entities.restore.review.error.target.must.be.ambiguity = Fehler: { $literal_target } muss ambiguity oder merge_candidate sein

journal.cli.sol.entities.restore.review.refusing.to.restore.review.item = Ohne { $option_yes } wird der Prüfungseintrag nicht wiederhergestellt.

journal.cli.sol.entities.restore.review.restored.review.item = Prüfungseintrag { $target } wiederhergestellt.

journal.cli.sol.entities.restore.version.refusing.to.restore.this.identity = Ohne { $option_yes } wird diese Identitätsversion nicht wiederhergestellt.

journal.cli.sol.entities.restore.version.restored.from.new.history.version = Wiederhergestellt: { $entity_id } aus { $version_id }; neue Verlaufsversion { $version }.

journal.cli.sol.entities.review.review.items.ambiguity.card.s = { $quantity ->
    [one] { $ambiguities_quantity ->
        [one] { $candidates_quantity ->
            [one] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte, { $ambiguities } zurückgestellte Mehrdeutigkeit, { $candidates } zurückgestellter Zusammenführungskandidat.
           *[other] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte, { $ambiguities } zurückgestellte Mehrdeutigkeit, { $candidates } zurückgestellter Zusammenführungskandidat(en).
        }
       *[other] { $candidates_quantity ->
            [one] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte, { $ambiguities } zurückgestellte Mehrdeutigkeit(en), { $candidates } zurückgestellter Zusammenführungskandidat.
           *[other] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte, { $ambiguities } zurückgestellte Mehrdeutigkeit(en), { $candidates } zurückgestellter Zusammenführungskandidat(en).
        }
    }
   *[other] { $ambiguities_quantity ->
        [one] { $candidates_quantity ->
            [one] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte(n), { $ambiguities } zurückgestellte Mehrdeutigkeit, { $candidates } zurückgestellter Zusammenführungskandidat.
           *[other] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte(n), { $ambiguities } zurückgestellte Mehrdeutigkeit, { $candidates } zurückgestellter Zusammenführungskandidat(en).
        }
       *[other] { $candidates_quantity ->
            [one] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte(n), { $ambiguities } zurückgestellte Mehrdeutigkeit(en), { $candidates } zurückgestellter Zusammenführungskandidat.
           *[other] Prüfungseinträge: { $cards } Mehrdeutigkeitskarte(n), { $ambiguities } zurückgestellte Mehrdeutigkeit(en), { $candidates } zurückgestellter Zusammenführungskandidat(en).
        }
    }
    }

journal.cli.sol.entities.search.activity.unreadable = Erkannte Entitätsaktivität ist unlesbar. Führ `{ $command }` aus, reparier den gemeldeten Datensatz, und versuch es noch einmal.

journal.cli.sol.entities.search.entities = { $quantity ->
    [one] { $count } Entität:
   *[other] { $count } Entitäten:
    }

journal.cli.sol.entities.search.facets = { $spacing_4 }Facetten: { $facets }

journal.cli.sol.entities.search.index.busy = Die Entitätssuche ist nicht verfügbar, solange die Indizierung läuft. Versuch es noch einmal, wenn sie fertig ist.

journal.cli.sol.entities.search.index.stale = Der Entitätssuchindex ist veraltet. Führ `{ $command }` aus und versuch es noch einmal.

journal.cli.sol.entities.search.index.unavailable = Der Entitätssuchindex ist nicht verfügbar. Führ `{ $command }` aus und versuch es noch einmal.

journal.cli.sol.entities.sweep.review.completed.entity.review.sweep = Entitätsprüfungsdurchlauf abgeschlossen.

journal.cli.sol.entities.sweep.review.refusing.to.run.review.sweep = Ohne { $option_yes } wird der Prüfungsdurchlauf nicht ausgeführt.

journal.cli.sol.entities.update.entity.updated = Entität '{ $resolved_name }' aktualisiert.

journal.cli.sol.entities.update.entity.updated.for = Entität '{ $entity }' für { $day } aktualisiert.

journal.cli.sol.health.activities.count = { $space_2 }{ $activities_count }: { $value }

journal.cli.sol.health.activities.user.edited = { $space_2 }{ $activities_user_edited }: { $value }

journal.cli.sol.health.activities.with.participation = { $space_2 }{ $activities_with_participation }: { $value }

journal.cli.sol.health.activities.with.story = { $space_2 }{ $activities_with_story }: { $value }

journal.cli.sol.health.argument.unexpected = Fehler: unerwartetes Argument { $token }.

journal.cli.sol.health.backlog.days = { $days_quantity ->
    [one] { $space_3 }über { $days } Tag hinweg
   *[other] { $space_3 }über { $days } Tage hinweg
    }

journal.cli.sol.health.backlog.days.incomplete = { $days_quantity ->
    [one] { $space_3 }über { $days } Tag hinweg (Status unvollständig)
   *[other] { $space_3 }über { $days } Tage hinweg (Status unvollständig)
    }

journal.cli.sol.health.backlog.media.days = { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $media } Segment wartet über { $days } Tag hinweg auf die Medienverarbeitung
   *[other] { $space_2 }{ $media } Segment wartet über { $days } Tage hinweg auf die Medienverarbeitung
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $media } Segmente warten über { $days } Tag hinweg auf die Medienverarbeitung
   *[other] { $space_2 }{ $media } Segmente warten über { $days } Tage hinweg auf die Medienverarbeitung
    }
    }

journal.cli.sol.health.backlog.media.days.incomplete = { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }mindestens { $media } Segment wartet über { $days } Tag hinweg auf die Medienverarbeitung (Status unvollständig)
   *[other] { $space_2 }mindestens { $media } Segment wartet über { $days } Tage hinweg auf die Medienverarbeitung (Status unvollständig)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }mindestens { $media } Segmente warten über { $days } Tag hinweg auf die Medienverarbeitung (Status unvollständig)
   *[other] { $space_2 }mindestens { $media } Segmente warten über { $days } Tage hinweg auf die Medienverarbeitung (Status unvollständig)
    }
    }

journal.cli.sol.health.backlog.thinking.days = { $thinking_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } Segment wartet über { $days } Tag hinweg auf das Denken
   *[other] { $space_2 }{ $thinking } Segment wartet über { $days } Tage hinweg auf das Denken
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } Segmente warten über { $days } Tag hinweg auf das Denken
   *[other] { $space_2 }{ $thinking } Segmente warten über { $days } Tage hinweg auf das Denken
    }
    }

journal.cli.sol.health.backlog.thinking.days.incomplete = { $thinking_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }mindestens { $thinking } Segment wartet über { $days } Tag hinweg auf das Denken (Status unvollständig)
   *[other] { $space_2 }mindestens { $thinking } Segment wartet über { $days } Tage hinweg auf das Denken (Status unvollständig)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }mindestens { $thinking } Segmente warten über { $days } Tag hinweg auf das Denken (Status unvollständig)
   *[other] { $space_2 }mindestens { $thinking } Segmente warten über { $days } Tage hinweg auf das Denken (Status unvollständig)
    }
    }

journal.cli.sol.health.backlog.thinking.media.days = { $thinking_quantity ->
    [one] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } Segment wartet auf das Denken, { $media } Segment wartet über { $days } Tag hinweg auf die Medienverarbeitung
   *[other] { $space_2 }{ $thinking } Segment wartet auf das Denken, { $media } Segment wartet über { $days } Tage hinweg auf die Medienverarbeitung
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } Segment wartet auf das Denken, { $media } Segmente warten über { $days } Tag hinweg auf die Medienverarbeitung
   *[other] { $space_2 }{ $thinking } Segment wartet auf das Denken, { $media } Segmente warten über { $days } Tage hinweg auf die Medienverarbeitung
    }
    }
   *[other] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }{ $thinking } Segmente warten auf das Denken, { $media } Segment wartet über { $days } Tag hinweg auf die Medienverarbeitung
   *[other] { $space_2 }{ $thinking } Segmente warten auf das Denken, { $media } Segment wartet über { $days } Tage hinweg auf die Medienverarbeitung
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }{ $thinking } Segmente warten auf das Denken, { $media } Segmente warten über { $days } Tag hinweg auf die Medienverarbeitung
   *[other] { $space_2 }{ $thinking } Segmente warten auf das Denken, { $media } Segmente warten über { $days } Tage hinweg auf die Medienverarbeitung
    }
    }
    }

journal.cli.sol.health.backlog.thinking.media.days.incomplete = { $thinking_quantity ->
    [one] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }mindestens { $thinking } Segment wartet auf das Denken, mindestens { $media } Segment wartet über { $days } Tag hinweg auf die Medienverarbeitung (Status unvollständig)
   *[other] { $space_2 }mindestens { $thinking } Segment wartet auf das Denken, mindestens { $media } Segment wartet über { $days } Tage hinweg auf die Medienverarbeitung (Status unvollständig)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }mindestens { $thinking } Segment wartet auf das Denken, mindestens { $media } Segmente warten über { $days } Tag hinweg auf die Medienverarbeitung (Status unvollständig)
   *[other] { $space_2 }mindestens { $thinking } Segment wartet auf das Denken, mindestens { $media } Segmente warten über { $days } Tage hinweg auf die Medienverarbeitung (Status unvollständig)
    }
    }
   *[other] { $media_quantity ->
    [one] { $days_quantity ->
    [one] { $space_2 }mindestens { $thinking } Segmente warten auf das Denken, mindestens { $media } Segment wartet über { $days } Tag hinweg auf die Medienverarbeitung (Status unvollständig)
   *[other] { $space_2 }mindestens { $thinking } Segmente warten auf das Denken, mindestens { $media } Segment wartet über { $days } Tage hinweg auf die Medienverarbeitung (Status unvollständig)
    }
   *[other] { $days_quantity ->
    [one] { $space_2 }mindestens { $thinking } Segmente warten auf das Denken, mindestens { $media } Segmente warten über { $days } Tag hinweg auf die Medienverarbeitung (Status unvollständig)
   *[other] { $space_2 }mindestens { $thinking } Segmente warten auf das Denken, mindestens { $media } Segmente warten über { $days } Tage hinweg auf die Medienverarbeitung (Status unvollständig)
    }
    }
    }

journal.cli.sol.health.backlog.unavailable = { $space_2 }Status der Segmentanalyse nicht verfügbar

journal.cli.sol.health.consumer = Verbrauchersignale

journal.cli.sol.health.coverage.ratio = { $space_2 }{ $coverage_ratio }: { $value }

journal.cli.sol.health.facets.silent.past.day = { $space_2 }{ $facets_silent_24h }: { $value }

journal.cli.sol.health.facets.with.recent.capture = { $space_2 }{ $facets_with_recent_capture }: { $value }

journal.cli.sol.health.hours.total = { $space_2 }{ $hours_total }: { $value }

journal.cli.sol.health.hours.with.capture = { $space_2 }{ $hours_with_capture }: { $value }

journal.cli.sol.health.index = { $space_2 }search_index: { $state } (veraltet { $stale }, fehlend { $missing }, verwaist { $orphaned }, fehlgeschlagen { $failed })

journal.cli.sol.health.index.activity.at = { $space_2 }{ $index_activity_at }: { $value }

journal.cli.sol.health.intake = Annahme

journal.cli.sol.health.last.segment.at = { $space_2 }{ $last_segment_at }: { $value }

journal.cli.sol.health.none = { $space_2 }nichts

journal.cli.sol.health.notes = Notizen

journal.cli.sol.health.profile.entities.total = { $space_2 }{ $profile_entities_total }: { $value }

journal.cli.sol.health.range = Bereich: { $start } -> { $end }

journal.cli.sol.health.silent = Detail stiller Facetten

journal.cli.sol.health.synthesis = Synthese

journal.cli.sol.health.talent.degraded.outputs.past.day = { $space_2 }{ $talent_degraded_outputs_24h }: { $value }

journal.cli.sol.health.talent.run.failures.past.day = { $space_2 }{ $talent_run_failures_24h }: { $value }

journal.cli.sol.help.apps = Apps ({ $invocation }):

journal.cli.sol.help.authority = Quelle: { $path }

journal.cli.sol.help.command.activities.create = Lege einen neuen synthetischen Aktivitätsdatensatz aus { $argv }-Flags oder aus { $json } auf { $stdin } an.

journal.cli.sol.help.command.activities.get = Hol einen Aktivitätsdatensatz anhand der ID.

journal.cli.sol.help.command.activities.list = Liste Aktivitätsdatensätze für einen Tag oder einen inklusiven Tagesbereich auf.

journal.cli.sol.help.command.activities.mute = Blende einen Aktivitätsdatensatz aus, ohne ihn zu löschen.

journal.cli.sol.help.command.activities.unmute = Stelle einen zuvor ausgeblendeten Aktivitätsdatensatz wieder her.

journal.cli.sol.help.command.activities.update = Wende ein { $argv }-Flag oder einen { $json }-Patch auf { $stdin } auf einen Aktivitätsdatensatz an.

journal.cli.sol.help.command.awareness.imports = Lies den Status der Importnachverfolgung oder aktualisiere ihn.

journal.cli.sol.help.command.awareness.log = Hänge einen Eintrag an das tägliche Wahrnehmungsprotokoll an.

journal.cli.sol.help.command.awareness.log.read = Lies Einträge aus dem täglichen Wahrnehmungsprotokoll.

journal.cli.sol.help.command.awareness.status = Zeige den aktuellen Zustand der Wahrnehmung.

journal.cli.sol.help.command.body.day = Zeige einen Tag importierter Gesundheitsdaten.

journal.cli.sol.help.command.body.status = Zeige den Status der importierten Gesundheitsdaten.

journal.cli.sol.help.command.body.window = Zeige den importierten Gesundheitskontext für ein Zeitfenster.

journal.cli.sol.help.command.entities.accept.merge.candidate = Zeige die Vorschau eines aufgezeichneten Kandidaten für eine Entitätszusammenführung oder nimm ihn an.

journal.cli.sol.help.command.entities.aka = Füge einer zugeordneten Entität einen Alias hinzu.

journal.cli.sol.help.command.entities.ambiguities = Liste prüfbare Mehrdeutigkeiten der Entitätsauflösung auf.

journal.cli.sol.help.command.entities.attach = Ordne eine Entität dauerhaft einer Facette zu.

journal.cli.sol.help.command.entities.delete = Verschiebe eine Entität und ihre Facettenverknüpfungen in den Papierkorb; { $erase_flag } löscht sie sofort endgültig. Mit { $erase_flag } entfernt { $from_backups_flag } sie auch aus deinen Sicherungen, und { $not_from_backups_flag } lässt sie dort. Ohne eines von beiden entscheidet die Papierkorbeinstellung.

journal.cli.sol.help.command.entities.detect = Zeichne eine erkannte Entität für einen Tag in einer Facette auf.

journal.cli.sol.help.command.entities.dismiss.merge.candidate = Verwirf einen aufgezeichneten Kandidaten für eine Entitätszusammenführung.

journal.cli.sol.help.command.entities.entity.history = Zeige dauerhafte Versionen der Identität (nicht den Beziehungsverlauf).

journal.cli.sol.help.command.entities.history = Zeige aufgezeichnete Verbindungsnachweise für ein Entitätspaar.

journal.cli.sol.help.command.entities.list = Liste Entitäten für eine Facette auf.

journal.cli.sol.help.command.entities.merge = Plane eine Zusammenführung von Entitäten des Journals oder schreibe sie fest.

journal.cli.sol.help.command.entities.merge.candidates = Liste aufgezeichnete Kandidaten für eine Entitätszusammenführung auf.

journal.cli.sol.help.command.entities.move = Verschiebe eine Entität von einer Facette in eine andere.

journal.cli.sol.help.command.entities.network = Zeige aufgezeichnete Verbindungen im Abstand einer Kante für eine Entität.

journal.cli.sol.help.command.entities.observations = Liste Beobachtungen für eine zugeordnete Entität auf.

journal.cli.sol.help.command.entities.observe = Füge einer zugeordneten Entität eine Beobachtung hinzu.

journal.cli.sol.help.command.entities.overview = Zeige eine globale Übersicht der aufgezeichneten Verbindungen.

journal.cli.sol.help.command.entities.record.merge.candidate = Zeichne eine vorgeschlagene Entitätszusammenführung auf (Quellvariante -> kanonisches Ziel).

journal.cli.sol.help.command.entities.resolve.ambiguity = Löse eine gespeicherte Wahl zur Mehrdeutigkeit auf oder ersetze sie.

journal.cli.sol.help.command.entities.resolve.ambiguity.group = Löse eine Kartengruppe zur Mehrdeutigkeit für einen Entitätskandidaten auf.

journal.cli.sol.help.command.entities.restore.review = Stelle ein zurückgestelltes Element der Prüfung wieder her oder mache eine automatische Wahl rückgängig.

journal.cli.sol.help.command.entities.restore.version = Stelle eine gewöhnliche Version der Identität wieder her.

journal.cli.sol.help.command.entities.review = Liste gruppierte Prüfungskarten zu Entitäten und zurückgestellte Elemente auf.

journal.cli.sol.help.command.entities.search = Suche Entitäten nach Text, Typ, Facette oder Aktivität.

journal.cli.sol.help.command.entities.sweep.review = Führe den Durchlauf der Richtlinie für die Entitätsprüfung aus.

journal.cli.sol.help.command.entities.update = Aktualisiere die Beschreibung einer Entität.

journal.cli.sol.help.command.facets.accept = Nimm einen Kandidaten der Facettenprüfung an.

journal.cli.sol.help.command.facets.dismiss = Verwirf einen Kandidaten der Facettenprüfung.

journal.cli.sol.help.command.facets.list.candidates = Liste aufgezeichnete Kandidaten der Facettenprüfung auf.

journal.cli.sol.help.command.health.for.range = Gib den Vertrauensbericht der Journaldaten für einen inklusiven Tagesbereich aus.

journal.cli.sol.help.command.health.full = Gib den vollständigen Vertrauensbericht der Journaldaten für einen Tag aus.

journal.cli.sol.help.command.health.pipeline = Dünner Wrapper um { $pipeline_module }. Für Vertrauensprüfungen der Journaldaten verwende `{ $summary_verb }` / `{ $full_verb }` / `{ $for_range_verb }`.

journal.cli.sol.help.command.health.summary = Fasse die Vertrauenssignale der Journaldaten für einen Tag zusammen.

journal.cli.sol.help.command.import.top.level = Importiere Medien durch das Journal

journal.cli.sol.help.command.journal.agents = Liste verfügbare Ausgaben von Agenten für einen Tag auf.

journal.cli.sol.help.command.journal.facet.create = Lege eine neue Facette an.{ $line_break }{ $line_break }{ $consent_flag } vermerkt im Prüfprotokoll der Facette, dass du das genehmigt hast. Ein{ $line_break }Agent übergibt es, nachdem du eine Änderung genehmigt hast, die der Agent vorgeschlagen hat, nicht für{ $line_break }eine, die du verlangt hast. Es ändert nicht, was angelegt wird.

journal.cli.sol.help.command.journal.facet.delete = Verschiebe eine Facette und alles darin in den Papierkorb, wo sie wiederhergestellt werden kann, bis sie endgültig gelöscht wird.{ $line_break }{ $line_break }{ $yes_flag } ist erforderlich; ohne das Flag verweigert der Befehl. Jedes Löschen wird im{ $line_break }Aktionsprotokoll des Journals als von dir genehmigt vermerkt, und { $consent_flag } wird akzeptiert, ändert aber{ $line_break }nichts.

journal.cli.sol.help.command.journal.facet.mute = Schalte eine Facette stumm (in den Standardlisten ausblenden).

journal.cli.sol.help.command.journal.facet.rename = Benenne eine Facette um.{ $line_break }{ $line_break }{ $consent_flag } vermerkt im Prüfprotokoll der Facette, dass du das genehmigt hast. Ein{ $line_break }Agent übergibt es, nachdem du eine Änderung genehmigt hast, die der Agent vorgeschlagen hat, nicht für{ $line_break }eine, die du verlangt hast. Es ändert die Umbenennung nicht.

journal.cli.sol.help.command.journal.facet.show = Zeige die Übersicht der Facette.

journal.cli.sol.help.command.journal.facet.unmute = Hebe die Stummschaltung einer Facette auf (in den Standardlisten anzeigen).

journal.cli.sol.help.command.journal.facet.update = Aktualisiere die Konfiguration der Facette.

journal.cli.sol.help.command.journal.facets = Liste Facetten auf.

journal.cli.sol.help.command.journal.import = Zeige die vollständigen Metadaten für einen einzelnen Import.

journal.cli.sol.help.command.journal.imports = Liste die letzten Importe mit Metadaten auf.

journal.cli.sol.help.command.journal.read = Lies den vollständigen Inhalt einer Agentenausgabe.

journal.cli.sol.help.command.journal.retention.config = Zeige die Aufbewahrungskonfiguration oder aktualisiere sie.

journal.cli.sol.help.command.journal.retention.list = Liste Originalmedien auf, die zur Entfernung bereit sind.

journal.cli.sol.help.command.journal.search = Durchsuche den Index des Journals.{ $line_break }{ $line_break }Verwende 2-4 Inhaltsbegriffe statt Fragen in natürlicher Sprache; Fragewörter{ $line_break }wie { $example_terms } erzeugen Rauschen in diesem Schlüsselwort/{ $bm25 }-Index. Syntax: { $or_operator } für{ $line_break }irgendeinen Begriff, Phrasen in Anführungszeichen für genauen Text und { $wildcard } für Präfixtreffer.{ $line_break }{ $n_flag }/{ $limit_flag } ist das Maximum an Ergebnissen pro Tag. Die Ergebnisse sind nach Tag gruppiert,{ $line_break }die neuesten zuerst, bis zu 20 Tage pro Seite. { $offset_flag } überspringt Tage. { $d_flag }/{ $day_flag } ist ein{ $line_break }genauer Tag und lässt sich nicht mit { $day_from_flag } oder { $day_to_flag } kombinieren. Wenn nichts{ $line_break }jeden Begriff innerhalb der anderen Filter (Facette, Agent, Stream, Zeit-{ $line_break }abschnitt) im ganzen Journal trifft, wird die Antwort mit { $relaxed } markiert und diese{ $line_break }Treffer sind schwächere Übereinstimmungen; dieses Urteil fällt vor dem Datumsfilter, sodass ein{ $line_break }leerer Tag oder Bereich nicht bedeutet, dass der Bereich die Begriffe nie nennt. Erweitere,{ $line_break }indem du Begriffe weglässt, dann { $or_operator }, dann { $wildcard }. { $json_flag } ändert die Ausgabe nicht,{ $line_break }weil sie so oder so { $json } ist.{ $line_break }Lies ein Ergebnis als zuletzt indexiert mit `{ $command }`. Kopiere alle drei Felder aus demselben Ergebnis; verwende seine Anzeige-ID nicht als Dateipfad.

journal.cli.sol.help.command.journal.storage.summary = Zeige die Speicherübersicht des Journals.

journal.cli.sol.help.command.link.authorized.clients = Liste jedes autorisierte Client-Zertifikat auf: Fingerabdruck, Bezeichnung, zuletzt gesehen (flache Ansicht).

journal.cli.sol.help.command.link.confirm = bestätige die mark deines Journals für einen Link, der darauf wartet

journal.cli.sol.help.command.link.join = nimm an { $solstone } mit einem Kurzcode oder Kopplungs-Link teil

journal.cli.sol.help.command.link.list = Gib jedes gekoppelte Gerät mit der Zeit aus, zu der es zuletzt gesehen wurde.

journal.cli.sol.help.command.link.local.network.close = Schließe dein Journal für Geräte in deinem Netzwerk.

journal.cli.sol.help.command.link.local.network.open = Öffne dein Journal für Geräte in deinem Netzwerk.

journal.cli.sol.help.command.link.local.network.status = Zeige, ob dein Journal für Geräte in deinem Netzwerk offen ist.

journal.cli.sol.help.command.link.observer.pause = pausiere verknüpfte Geräte (noch nicht verfügbar).

journal.cli.sol.help.command.link.pair = Starte einen Kopplungs-Link, gib beitrittsbereite Zugangsdaten aus und warte optional.

journal.cli.sol.help.command.link.private.link.disable = Schalte das private Netzwerk aus.

journal.cli.sol.help.command.link.private.link.setup = Richte das private Netzwerk ein.

journal.cli.sol.help.command.link.private.link.status = Zeige den Status des privaten Netzwerks.

journal.cli.sol.help.command.link.serve = stelle ein gekoppeltes Journal über die lokale Link-Brücke bereit

journal.cli.sol.help.command.link.set.label = Setze oder entferne deine Bezeichnung für ein gekoppeltes Gerät.

journal.cli.sol.help.command.link.status = Melde die Registrierung, den Zustand von { $listen_ws }, die Anzahl aktiver Tunnel und die Relay-Adresse.

journal.cli.sol.help.command.link.top.level.status = zeige den Link-Status und die beobachtete Version des entfernten Journals

journal.cli.sol.help.command.link.unpair = Widerrufe ein gekoppeltes Gerät. Die nächste Wiederverbindung von diesem Gerät scheitert beim { $tls }-Handshake.

journal.cli.sol.help.command.moved.navigate = Verschoben nach `{ $command }`.

journal.cli.sol.help.command.settings.convey.status = Zeige den Status des { $convey }-Netzwerks und der Dashboard-URL.

journal.cli.sol.help.command.settings.identity.set = Aktualisiere deine Identität im Journal.

journal.cli.sol.help.command.settings.identity.show = Zeige die Identitätskonfiguration des Journals.

journal.cli.sol.help.command.settings.keys.clear = Entferne einen API-Schlüssel aus der Journal-Konfiguration.

journal.cli.sol.help.command.settings.keys.set = Setze einen API-Schlüssel in der Journal-Konfiguration.

journal.cli.sol.help.command.settings.keys.show = Zeige den Status der konfigurierten API-Schlüssel.

journal.cli.sol.help.command.settings.keys.validate = Überprüfe alle konfigurierten API-Schlüssel, ohne sie standardmäßig zu speichern.

journal.cli.sol.help.command.settings.processing.set = Setze den Modus der aufgeschobenen Verarbeitung und das Abarbeitungsfenster.

journal.cli.sol.help.command.settings.processing.show = Zeige die wirksamen Einstellungen der aufgeschobenen Verarbeitung.

journal.cli.sol.help.command.settings.show = Zeige eine Übersicht der Journal-Einstellungen.

journal.cli.sol.help.command.settings.transcribe.set.backend = Setze das Transkriptions-Backend.

journal.cli.sol.help.command.settings.transcribe.show = Zeige die Konfiguration des Transkriptions-Backends.

journal.cli.sol.help.command.speakers.attribute.segment = Führe die Sprecherzuschreibung (Schichten 1-3) für ein einzelnes Segment aus.{ $line_break }{ $line_break }Klassifiziert jeden Satz anhand der Erkennung von dir, struktureller Heuristiken{ $line_break }und des akustischen Abgleichs der Stimmabdrücke. Schreibt optional { $labels_file }{ $line_break }und sammelt Stimmabdrücke mit hoher Sicherheit.

journal.cli.sol.help.command.speakers.backfill = Führe die Sprecherzuschreibung über alle Segmente mit Einbettungen aus.{ $line_break }{ $line_break }Verarbeitet die Segmente vom ältesten her für den schrittweisen Aufbau der Stimmabdrücke.{ $line_break }Überspringt Segmente, die bereits { $labels_file } haben (ein erneuter Lauf ist sicher).

journal.cli.sol.help.command.speakers.backfill.last.seen = Fülle { $last_seen_field } in bestehenden Metadatenzeilen der Stimmabdrücke nach.

journal.cli.sol.help.command.speakers.backfill.resume = Setze einen unterbrochenen oder fehlgeschlagenen Nachfüllvorgang der Sprecherzuschreibung fort.

journal.cli.sol.help.command.speakers.backfill.status = Zeige den Status eines Nachfüllvorgangs der Sprecherzuschreibung.

journal.cli.sol.help.command.speakers.bootstrap = Lege Stimmabdrücke aus Segmenten mit genau einem Sprecher an.{ $line_break }{ $line_break }Durchsucht das ganze Journal nach Segmenten, in denen { $speakers_file } genau{ $line_break }einen Sprecher nennt. In diesen Segmenten gehören alle Einbettungen, die nicht von dir sind, zu dieser{ $line_break }Person. Speichert sie als Stimmabdrücke und nutzt deinen Zentroid für{ $line_break }die Subtraktion von dir.

journal.cli.sol.help.command.speakers.build.from.tags = Baue den Zentroid deiner Stimme aus überprüften manuellen Markierungen von dir auf.

journal.cli.sol.help.command.speakers.confirm.owner = Bestätige den Kandidaten für deine Stimme im Browser.{ $line_break }{ $line_break }{ $confirm_verb } ist nativ und speichert keinen Zentroid. Prüfe die Kandidaten und bestätige sie im Browser unter { $speakers_page }.

journal.cli.sol.help.command.speakers.correct = Korrigiere eine Sprecherzuschreibung sofort.

journal.cli.sol.help.command.speakers.day.segments = Liste Segmente der Sprecherprüfung für einen Tag auf.

journal.cli.sol.help.command.speakers.detect = Führe die Erkennung von Kandidaten für deine Stimme aus.

journal.cli.sol.help.command.speakers.discover = Entdecke wiederkehrende unbekannte Sprecher über Segmente hinweg.

journal.cli.sol.help.command.speakers.dismiss.cluster = Verwirf einen aktuellen Entdeckungscluster aus den Listenoberflächen.

journal.cli.sol.help.command.speakers.dismissals = Liste zusammengefasste Verwerfungen von Sprecherclustern auf.

journal.cli.sol.help.command.speakers.identify = Identifiziere einen entdeckten Cluster eines unbekannten Sprechers.

journal.cli.sol.help.command.speakers.identify.operation = Zeige einen geschwärzten Vorgang zur Sprecheridentifikation.

journal.cli.sol.help.command.speakers.identify.operations = Liste geschwärzte Vorgänge zur Sprecheridentifikation auf.

journal.cli.sol.help.command.speakers.identify.undo = Mache einen festgeschriebenen Vorgang zur Sprecheridentifikation rückgängig.

journal.cli.sol.help.command.speakers.keep.separate.list = Liste zusammengefasste Getrennt-halten-Aussagen zu Sprechern auf.

journal.cli.sol.help.command.speakers.link.import = Verknüpfe den Namen einer teilnehmenden Person aus einem Import als Alias mit einer bestehenden Entität.

journal.cli.sol.help.command.speakers.merge.names = Führe eine Namensvariante eines Sprechers in eine kanonische Entität zusammen.

journal.cli.sol.help.command.speakers.owner.ready = Prüfe, ob die Erkennung deiner Stimme dir angezeigt werden soll.

journal.cli.sol.help.command.speakers.presence = Zeige, wer um einen entdeckten Cluster einer unbekannten Stimme herum war.

journal.cli.sol.help.command.speakers.propagate.correction = Zeige die Vorschau der begrenzten Weitergabe einer Korrektur oder wende sie an.{ $line_break }{ $line_break }Die Umkehrung nutzt dasselbe Verb, wobei die beiden Sprecher-IDs vertauscht sind.

journal.cli.sol.help.command.speakers.rebuild.owner = Baue den Zentroid deiner Stimme aus den aktuellen manuellen Markierungen von dir neu auf.

journal.cli.sol.help.command.speakers.reject.owner = Lehne die dargestellte Generation des Kandidaten für deine Stimme ab.

journal.cli.sol.help.command.speakers.repair = Repariere Verunreinigung durch Nicht-{ $person_type }-Sprecher, ohne daraus neu zu lernen.{ $line_break }{ $line_break }Untersucht Identitäten und Stimmabdrücke, entfernt Verunreinigung von Stimmabdrücken durch Nicht-{ $person_type },{ $line_break }und aktualisiert die Segmentbezeichnungen mit frischer Zuschreibung im aktuellen Zustand.

journal.cli.sol.help.command.speakers.repair.resume = Setze einen unterbrochenen oder fehlgeschlagenen Reparaturvorgang für Sprecher fort.

journal.cli.sol.help.command.speakers.repair.status = Zeige den Status eines Reparaturvorgangs für Sprecher.

journal.cli.sol.help.command.speakers.resolve.names = Löse Namensvarianten von Sprechern anhand der Ähnlichkeit von Stimmabdrücken auf.{ $line_break }{ $line_break }Vergleicht die Zentroide der Stimmabdrücke zwischen allen Entitäten. Paare mit einer Kosinus-{ $line_break }Ähnlichkeit > 0.90 werden als dieselbe Person markiert. Eindeutige Varianten{ $line_break }(der Kurzname ist das erste Wort des vollen Namens) werden automatisch zusammengeführt, indem der{ $line_break }Kurzname als Alias an der kanonischen Entität hinzugefügt wird.

journal.cli.sol.help.command.speakers.seed.from.imports = Säe Stimmabdrücke aus Importsegmenten mit sprecherzugeordneten Transkripten.{ $line_break }{ $line_break }Durchsucht Import-Streams nach Segmenten mit sowohl { $transcript_file }{ $line_break }(mit Sprecherbezeichnungen) als auch Audio-Einbettungen. Ordnet jede Einbettung einem Sprecher zu,{ $line_break }über eine zeitbasierte Ausrichtung, gleicht Sprecher mit bestehenden Entitäten ab und speichert{ $line_break }die Einbettungen als Stimmabdrücke mit Schutz vor Verunreinigung durch dich.

journal.cli.sol.help.command.speakers.sentences = Liste Satz-IDs für eine Quelle der Sprecherprüfung auf.

journal.cli.sol.help.command.speakers.status = Zeige den Status des Sprecher-Subsystems als { $json }.

journal.cli.sol.help.command.speakers.suggest = Schlage Gelegenheiten zur Pflege der Sprecher vor.

journal.cli.sol.help.command.speakers.tag.owner = Markiere einen Satz als deine Stimme.

journal.cli.sol.help.command.speakers.wipe = Verschiebe jede Datei einer Sprecherstimme im Journal in den Papierkorb.{ $line_break }{ $line_break }Ohne { $commit_flag } gibt der Befehl einen Bericht aus, was verschoben würde. Mit{ $line_break }{ $commit_flag } verschiebt er NPZs der Segment-Einbettungen, Sprecherbezeichnungen/Korrekturen,{ $line_break }Stimmabdrücke je Entität, deine Zentroide, das Abbild des Kandidaten für dich{ $line_break }und den Kandidatenpool in den Papierkorb, wo sie wiederhergestellt werden können, bis{ $line_break }sie endgültig gelöscht werden. { $erase_flag } löscht sie stattdessen sofort endgültig.{ $line_break }{ $line_break }Mit { $erase_flag } entfernt { $from_backups_flag } sie auch aus deinen Sicherungen und{ $line_break }{ $not_from_backups_flag } lässt sie dort. Ohne eines von beiden entscheidet die Papierkorb-{ $line_break }Einstellung.

journal.cli.sol.help.command.status.top.level = Zeige den Netzwerkstatus des Journals.

journal.cli.sol.help.command.thinking.clear.local.endpoint = Entferne die Adresse des lokalen BYO-Anbieters.

journal.cli.sol.help.command.thinking.confidential.disable = Deaktiviere die vertrauliche Verarbeitung.

journal.cli.sol.help.command.thinking.confidential.enable = Aktiviere die vertrauliche Verarbeitung.

journal.cli.sol.help.command.thinking.confidential.recheck = Prüfe die vertrauliche Verarbeitung erneut.

journal.cli.sol.help.command.thinking.confidential.status = Zeige den Status der vertraulichen Verarbeitung.

journal.cli.sol.help.command.thinking.keys.clear = Entferne einen KI-Schlüssel aus der Journal-Konfiguration.

journal.cli.sol.help.command.thinking.keys.set = Setze einen KI-Schlüssel in der Journal-Konfiguration.

journal.cli.sol.help.command.thinking.keys.show = Zeige den Status der konfigurierten KI-Schlüssel.

journal.cli.sol.help.command.thinking.keys.validate = Überprüfe die konfigurierten KI-Schlüssel.

journal.cli.sol.help.command.thinking.local.availability = Zeige die Verfügbarkeit der lokalen Modelle.

journal.cli.sol.help.command.thinking.local.bootstrap = Starte die Einrichtung des lokalen Modells.

journal.cli.sol.help.command.thinking.local.bootstrap.cancel = Brich die Einrichtung des lokalen Modells ab.

journal.cli.sol.help.command.thinking.local.bootstrap.status = Zeige den Status der lokalen Einrichtung.

journal.cli.sol.help.command.thinking.local.models = Liste die lokalen Modelle auf.

journal.cli.sol.help.command.thinking.local.readiness = Zeige die Bereitschaft des lokalen Anbieters.

journal.cli.sol.help.command.thinking.local.status = Zeige den Status des lokalen Anbieters.

journal.cli.sol.help.command.thinking.providers.set.active = Setze den Anbieter und das Modell, die für das gesamte Denken verwendet werden.

journal.cli.sol.help.command.thinking.providers.show = Zeige die Konfiguration des Anbieters.

journal.cli.sol.help.command.thinking.set.local.endpoint = Setze die Adresse des lokalen BYO-Anbieters.

journal.cli.sol.help.command.transcripts.read = Lies den Transkriptinhalt für einen Tag, ein Segment oder einen Zeitraum.

journal.cli.sol.help.command.transcripts.scan = Liste die Abdeckungsbereiche des Transkripts für einen Tag auf.

journal.cli.sol.help.command.transcripts.segments = Liste die Aufnahmesegmente für einen Tag auf.

journal.cli.sol.help.command.transcripts.speakers = Liste die Korrekturzeilen der Sprecher für ein Transkriptsegment auf.

journal.cli.sol.help.command.transcripts.stats = Zeige die täglichen Abdeckungszahlen des Transkripts für einen Monat.

journal.cli.sol.help.command.usage = Verwendung: { $syntax }

journal.cli.sol.help.computer = Dieser Computer

journal.cli.sol.help.journal = Dein Journal

journal.cli.sol.help.operation = Vorgang: { $operation }

journal.cli.sol.help.parameter = { $shape ->
    *[count] { $syntax } [Anzahl]
    [multiple] { $syntax } [mehrere]
    [multiple_count] { $syntax } [mehrere, Anzahl]
    [required] { $syntax } [erforderlich]
    [required_count] { $syntax } [erforderlich, Anzahl]
    [required_multiple] { $syntax } [erforderlich, mehrere]
    [required_multiple_count] { $syntax } [erforderlich, mehrere, Anzahl]
    }

journal.cli.sol.help.parameter.default = { $shape ->
    *[plain] { $syntax } [Standard: { $default_value }]
    [count] { $syntax } [Standard: { $default_value }, Anzahl]
    [multiple] { $syntax } [Standard: { $default_value }, mehrere]
    [multiple_count] { $syntax } [Standard: { $default_value }, mehrere, Anzahl]
    [required] { $syntax } [erforderlich, Standard: { $default_value }]
    [required_count] { $syntax } [erforderlich, Standard: { $default_value }, Anzahl]
    [required_multiple] { $syntax } [erforderlich, Standard: { $default_value }, mehrere]
    [required_multiple_count] { $syntax } [erforderlich, Standard: { $default_value }, mehrere, Anzahl]
    }

journal.cli.sol.help.parameter.default.flag = { $shape ->
    *[plain] { $syntax } [Standard: { $default_value }, Flag-Wert: { $flag_value }]
    [count] { $syntax } [Standard: { $default_value }, Flag-Wert: { $flag_value }, Anzahl]
    [multiple] { $syntax } [Standard: { $default_value }, Flag-Wert: { $flag_value }, mehrere]
    [multiple_count] { $syntax } [Standard: { $default_value }, Flag-Wert: { $flag_value }, mehrere, Anzahl]
    [required] { $syntax } [erforderlich, Standard: { $default_value }, Flag-Wert: { $flag_value }]
    [required_count] { $syntax } [erforderlich, Standard: { $default_value }, Flag-Wert: { $flag_value }, Anzahl]
    [required_multiple] { $syntax } [erforderlich, Standard: { $default_value }, Flag-Wert: { $flag_value }, mehrere]
    [required_multiple_count] { $syntax } [erforderlich, Standard: { $default_value }, Flag-Wert: { $flag_value }, mehrere, Anzahl]
    }

journal.cli.sol.help.parameter.flag = { $shape ->
    *[plain] { $syntax } [Flag-Wert: { $flag_value }]
    [count] { $syntax } [Flag-Wert: { $flag_value }, Anzahl]
    [multiple] { $syntax } [Flag-Wert: { $flag_value }, mehrere]
    [multiple_count] { $syntax } [Flag-Wert: { $flag_value }, mehrere, Anzahl]
    [required] { $syntax } [erforderlich, Flag-Wert: { $flag_value }]
    [required_count] { $syntax } [erforderlich, Flag-Wert: { $flag_value }, Anzahl]
    [required_multiple] { $syntax } [erforderlich, Flag-Wert: { $flag_value }, mehrere]
    [required_multiple_count] { $syntax } [erforderlich, Flag-Wert: { $flag_value }, mehrere, Anzahl]
    }

journal.cli.sol.help.section.arguments = Argumente:

journal.cli.sol.help.section.commands = Befehle:

journal.cli.sol.help.section.options = Optionen:

journal.cli.sol.help.title = { $solstone_name } - dein Journal, von der Befehlszeile

journal.cli.sol.help.tools = Werkzeuge

journal.cli.sol.import.already.started = bereits gestartet{ $line_break }

journal.cli.sol.import.argument.media = die folgenden Argumente sind erforderlich: media

journal.cli.sol.import.argument.unexpected = unerwartete(s) Argument(e): { $arguments }

journal.cli.sol.import.argument.unknown = nicht erkannte Argumente: { $token }

journal.cli.sol.import.duplicate.entries = { $entries_quantity ->
    [one] { $solstone_import }: bereits am { $date } importiert ({ $entries } Eintrag); wird übersprungen
   *[other] { $solstone_import }: bereits am { $date } importiert ({ $entries } Einträge); wird übersprungen
    }

journal.cli.sol.import.duplicate.entries.literal = { $solstone_import }: bereits am { $date } importiert ({ $entries } Einträge); wird übersprungen{ $line_break }

journal.cli.sol.import.duplicate.entries.literal.unknown = { $solstone_import }: bereits an unbekanntem Datum importiert ({ $entries } Einträge); wird übersprungen{ $line_break }

journal.cli.sol.import.duplicate.entries.unknown = { $entries_quantity ->
    [one] { $solstone_import }: bereits an unbekanntem Datum importiert ({ $entries } Eintrag); wird übersprungen
   *[other] { $solstone_import }: bereits an unbekanntem Datum importiert ({ $entries } Einträge); wird übersprungen
    }

journal.cli.sol.import.duplicate.imported = { $solstone_import }: bereits am { $date } importiert; wird übersprungen{ $line_break }

journal.cli.sol.import.duplicate.imported.unknown = { $solstone_import }: bereits an unbekanntem Datum importiert; wird übersprungen{ $line_break }

journal.cli.sol.import.duplicate.skipping = { $solstone_import }: doppelter Import; wird übersprungen{ $line_break }

journal.cli.sol.import.duplicate.staged = { $solstone_import }: bereits als { $import_id } bereitgestellt; wird übersprungen{ $line_break }

journal.cli.sol.import.duplicate.staged.unknown = { $solstone_import }: bereits als unbekannt bereitgestellt; wird übersprungen{ $line_break }

journal.cli.sol.import.help = Verwendung: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Importiere Medien durch das Journal{ $line_break }

journal.cli.sol.import.host.guidance = `{ $auto } { $guidance }` erfordert den Journal-Host. Verwende hier `{ $timestamp }` oder führe `{ $solstone_journal_importer }` aus.

journal.cli.sol.import.host.option = `{ $option }` erfordert den Journal-Host. Führe das auf dem Journal-Host mit `{ $solstone_journal_importer }` aus.

journal.cli.sol.import.journal.unreachable = { $solstone_import }: das Journal konnte nicht erreicht werden. Starte es mit '{ $solstone_journal_up }' und versuch es noch einmal.{ $line_break }

journal.cli.sol.import.notice.failed = der Hinweis zum Importtag konnte nicht angezeigt werden: { $error }

journal.cli.sol.import.queued = Verarbeitung in der Warteschlange{ $line_break }

journal.cli.sol.import.response.malformed = { $solstone_import }: die Antwort des Journals konnte nicht gelesen werden{ $line_break }

journal.cli.sol.import.size.failed = die Importgröße konnte nicht angezeigt werden: { $error }

journal.cli.sol.import.size.skipped = { $skipped } übersprungen{ $line_break }

journal.cli.sol.import.size.total = { $recordings_quantity ->
    [one] { $recordings } Aufnahme über { $days_quantity ->
    [one] { $days } Tag hinweg
   *[other] { $days } Tage hinweg
    }
   *[other] { $recordings } Aufnahmen über { $days_quantity ->
    [one] { $days } Tag hinweg
   *[other] { $days } Tage hinweg
    }
    }

journal.cli.sol.import.stage.failed = { $solstone_import }: der Import konnte nicht bereitgestellt werden: { $error }{ $line_break }

journal.cli.sol.import.staged.failed = { $solstone_import }: { $staged_path } bereitgestellt, aber die Verarbeitung wurde nicht in die Warteschlange gestellt: { $error }{ $line_break }

journal.cli.sol.import.staged.malformed = { $solstone_import }: { $staged_path } bereitgestellt, aber die Verarbeitung wurde nicht in die Warteschlange gestellt: die Antwort des Journals konnte nicht gelesen werden{ $line_break }

journal.cli.sol.import.staged.path = bereitgestellt { $path }{ $line_break }

journal.cli.sol.import.staged.timestamp = Zeitstempel { $timestamp }{ $line_break }

journal.cli.sol.import.staged.unreachable = { $solstone_import }: { $staged_path } bereitgestellt, aber die Verarbeitung wurde nicht in die Warteschlange gestellt: das Journal konnte nicht erreicht werden{ $line_break }

journal.cli.sol.import.task.queued = Verarbeitungsaufgabe { $task_id } in der Warteschlange{ $line_break }

journal.cli.sol.import.task.started = bereits als Aufgabe { $task_id } gestartet{ $line_break }

journal.cli.sol.import.usage.error = Verwendung: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Importiere Medien durch das Journal{ $line_break }{ $solstone_import }: Fehler: { $error }{ $line_break }

journal.cli.sol.journal.facet.delete.yes = Fehler: { $yes } ist erforderlich, um eine Facette zu löschen.

journal.cli.sol.launcher.cycle.mac = { $solstone_name }: nativer Symlink-Zyklus des Starters erkannt. Installiere die Journal-App neu.

journal.cli.sol.launcher.cycle.unix = { $solstone_name }: nativer Symlink-Zyklus des Starters erkannt. Installiere { $solstone_name } und { $sibling } neu.

journal.cli.sol.launcher.dangling.mac = { $solstone_name }: der native Symlink des Starters zeigt ins Leere: { $path }. Installiere die Journal-App neu.

journal.cli.sol.launcher.dangling.unix = { $solstone_name }: der native Symlink des Starters zeigt ins Leere: { $path }. Installiere { $solstone_name } und { $sibling } neu.

journal.cli.sol.launcher.executable.mac = { $solstone_name }: das native Gegenstück { $sibling } ist nicht ausführbar: { $path }. Installiere die Journal-App neu.

journal.cli.sol.launcher.executable.unix = { $solstone_name }: das native Gegenstück { $sibling } ist nicht ausführbar: { $path }. Installiere { $solstone_name } und { $sibling } neu.

journal.cli.sol.launcher.missing.mac = { $solstone_name }: das native Gegenstück { $sibling } fehlt: { $path }. Installiere die Journal-App neu.

journal.cli.sol.launcher.missing.unix = { $solstone_name }: das native Gegenstück { $sibling } fehlt: { $path }. Installiere { $solstone_name } und { $sibling } neu.

journal.cli.sol.link.argument.expected = Argument { $option }: erwartet ein Argument

journal.cli.sol.link.argument.unrecognized = unerkannte Argumente: { $unknown }

journal.cli.sol.link.bundle.chain.empty = { $chain_pem } enthält keine Zertifikate

journal.cli.sol.link.bundle.chain.incomplete = { $chain_pem } enthält ein unvollständiges Zertifikat

journal.cli.sol.link.bundle.endpoint.ip = { $lan } endpoint ohne ip

journal.cli.sol.link.bundle.endpoint.port = { $lan } endpoint hat einen ungültigen Port

journal.cli.sol.link.bundle.endpoints.list = { $peer_json } { $local_endpoints } muss eine Liste sein

journal.cli.sol.link.bundle.endpoints.objects = { $peer_json } { $local_endpoints } Einträge müssen Objekte sein

journal.cli.sol.link.bundle.exists = Der Pfad der Zugangsdaten existiert bereits: { $path }. Entferne ihn und führe den Befehl erneut aus, wenn du erneut koppeln willst.

journal.cli.sol.link.bundle.exists.spent = Der Pfad der Zugangsdaten existiert bereits: { $path }. Der Kopplungscode ist jetzt verbraucht; erzeuge einen neuen und führe den Befehl nach dem Entfernen erneut aus.

journal.cli.sol.link.bundle.files.incomplete = der Dateisatz des Zugangsdaten-Bundles ist unvollständig

journal.cli.sol.link.bundle.files.missing = { $pl }-Bundle-Datei fehlt: { $paths }

journal.cli.sol.link.bundle.home.missing = das Home-Verzeichnis für die Kopplungszugangsdaten dieses Geräts konnte nicht aufgelöst werden.

journal.cli.sol.link.bundle.invalid = ungültiges Link-Bundle für das Label '{ $label }' unter { $path }: { $error }. Führe `{ $solstone_link_join }` aus, um dieses Gerät zu koppeln.

journal.cli.sol.link.bundle.multiple = mehrere Link-Bundles gefunden: { $labels }. übergib { $label }, um eines zu wählen.

journal.cli.sol.link.bundle.none = keine Link-Bundles unter { $path } gefunden. führe `{ $solstone_link_join }` aus, um dieses Gerät zu koppeln.

journal.cli.sol.link.bundle.path.missing = { $pl }-Bundle nicht gefunden: { $path }

journal.cli.sol.link.bundle.peer.invalid = { $peer_json } in { $path } ist ungültig: { $error }

journal.cli.sol.link.confirm.done = die mark deines Journals ist bereits bestätigt. nichts zu tun.{ $line_break }

journal.cli.sol.link.confirm.error = { $solstone_link_confirm }: Fehler: { $error }{ $line_break }

journal.cli.sol.link.confirm.help = Verwendung: { $solstone_link_confirm_h_label_ }{ $line_break }{ $line_break }Bestätige die mark des Journals für einen Link, der darauf wartet.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $h_help }{ $space_5 }diese Hilfe anzeigen und beenden{ $line_break }{ $label_label }{ $space_2 }Link-Bundle-Label (standardmäßig der einzige gekoppelte Link){ $line_break }{ $mark_mark }{ $space_4 }die zwei Wörter der mark, die die Netzwerk-App deines Journals anzeigt.{ $line_break }{ $space_17 }nötig, wenn kein Terminal da ist, um dich zu fragen.{ $line_break }

journal.cli.sol.link.confirm.success = gekoppelt.{ $line_break }

journal.cli.sol.link.confirm.usage.error = Verwendung: { $solstone_link_confirm_h_label_ }{ $line_break }{ $solstone_link_confirm }: Fehler: { $error }{ $line_break }

journal.cli.sol.link.held = wartet darauf, dass du die mark deines Journals bestätigst. nichts, was wartet, geht in dein Journal, bis du das tust.{ $line_break }wenn du bereit bist, führe aus: { $solstone_link_confirm } { $label_1 } { $label }{ $line_break }

journal.cli.sol.link.join.code.invalid = Der Kopplungscode entsprach keiner akzeptierten Form. Verwende einen Kopplungs-Link wie { $https_go_solstone_app_p }... aus '{ $solstone_call_link_pair }'.

journal.cli.sol.link.join.code.malformed = Fehlerhafter Kopplungs-Link. Verwende den vollständigen Wert { $https_go_solstone_app_p }... aus der Kopplungsausgabe.

journal.cli.sol.link.join.code.nonlocal = Der Kopplungs-Link zeigt auf eine Adresse außerhalb des lokalen Netzwerks, die dieser Beitritt anwählt.

journal.cli.sol.link.join.help = Verwendung: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $h_help }{ $space_5 }diese Hilfe anzeigen und beenden{ $line_break }{ $home_home }{ $space_4 }Empfänger-Basis { $url }{ $line_break }{ $code_code }{ $space_4 }Kopplungs-Link { $url }{ $line_break }{ $as_as_role }{ $space_3 }Optionales Tag, unter dem beigetreten wird{ $line_break }{ $label_label }{ $space_2 }Lokales Zugangsdaten-Label (standardmäßig der Hostname dieses Computers){ $line_break }{ $mark_mark }{ $space_4 }die zwei Wörter der mark, die die Netzwerk-App deines Journals anzeigt.{ $line_break }{ $space_17 }nötig, wenn kein Terminal da ist, um dich zu fragen.{ $line_break }

journal.cli.sol.link.join.invalid.role = ungültige Rolle; erwartet wird eines von: { $phone }, { $observer }{ $line_break }

journal.cli.sol.link.join.lock.error = { $solstone_link_join }: Fehler: { $err }{ $line_break }

journal.cli.sol.link.join.paired = gekoppelt.{ $line_break }Zugangsdaten: { $path }{ $line_break }

journal.cli.sol.link.join.pairing.unavailable = Die Link-Kopplungsnaht ist nicht verfügbar.{ $line_break }

journal.cli.sol.link.join.required.code = die folgenden Argumente sind erforderlich: { $code }

journal.cli.sol.link.join.response.large = Die Kopplungsantwort { $local_endpoints } ist zu groß.{ $line_break }

journal.cli.sol.link.join.target.host = Ziel des Kopplungs-Links ohne Host.

journal.cli.sol.link.join.target.port = Ziel des Kopplungs-Links ohne expliziten Port.

journal.cli.sol.link.join.usage.error = Verwendung: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $solstone_link_join }: Fehler: { $error }{ $line_break }

journal.cli.sol.link.label.character = { $label } darf nur Buchstaben, Ziffern, '-', '_' und '.' enthalten

journal.cli.sol.link.label.empty = { $label } darf nicht leer sein

journal.cli.sol.link.label.length = { $label } muss 80 Zeichen oder weniger sein

journal.cli.sol.link.label.separator = { $label } darf keine Pfadtrennzeichen enthalten

journal.cli.sol.link.label.start = { $label } darf nicht mit '.' beginnen

journal.cli.sol.link.label.traversal = { $label } darf nicht '..' enthalten

journal.cli.sol.link.mark.ask.identified = stimmt das mit deinem Journal überein? gib { $yes } oder { $no } ein:

journal.cli.sol.link.mark.ask.unavailable = gib { $continue } ein, um trotzdem zu koppeln, oder { $cancel }, um anzuhalten:

journal.cli.sol.link.mark.confirm.cancel = Kopplung abgebrochen. hol einen frischen Kopplungs-Link von deinem Journal und führe { $solstone_link_join } erneut aus, wenn du bereit bist.{ $line_break }

journal.cli.sol.link.mark.confirm.mismatch = nicht gekoppelt.{ $line_break }du hast gesagt, dass diese mark nicht mit der übereinstimmt, die dein Journal anzeigt, dieser Computer ist also nicht gekoppelt. du hast vielleicht den falschen Link eingefügt, oder etwas stimmt nicht. hol einen frischen Kopplungs-Link von deinem Journal und führe { $solstone_link_join } erneut aus, oder schreib an { $support_solstone_app } und wir helfen.{ $line_break }

journal.cli.sol.link.mark.confirm.terminal = { $solstone_link_confirm } kann dich hier nicht nach der mark deines Journals fragen. führe es in einem Terminal aus oder gib die zwei Wörter der mark mit { $mark_word_word } an.{ $line_break }

journal.cli.sol.link.mark.confirm.unavailable = die mark des Journals konnte von diesem Computer nicht ermittelt werden, die Wörter, die du angegeben hast, lassen sich also nicht vergleichen. wartet darauf, dass du die mark deines Journals bestätigst. nichts, was wartet, geht in dein Journal, bis du das tust. führe { $solstone_link_confirm } in einem Terminal aus, um selbst zu entscheiden.{ $line_break }

journal.cli.sol.link.mark.confirm.wrong = nicht gekoppelt.{ $line_break }die Wörter der mark, die du angegeben hast, stimmen nicht mit der mark des Journals überein, dieser Computer ist also nicht gekoppelt. prüfe die Wörter, hol einen frischen Kopplungs-Link von deinem Journal und führe { $solstone_link_join } erneut aus.{ $line_break }

journal.cli.sol.link.mark.join.cancel = Kopplung abgebrochen. nichts, was dieser Computer behalten hat, ging über diesen Link an jenes Journal. hol einen frischen Kopplungs-Link von deinem Journal und führe { $solstone_link_join } erneut aus, wenn du bereit bist.{ $line_break }

journal.cli.sol.link.mark.join.mismatch = nicht gekoppelt.{ $line_break }du hast gesagt, dass diese mark nicht mit der übereinstimmt, die dein Journal anzeigt, dieser Computer ist also nicht gekoppelt, und nichts, was er behalten hat, ging über diesen Link an jenes Journal. du hast vielleicht den falschen Link eingefügt, oder etwas stimmt nicht. hol einen frischen Kopplungs-Link von deinem Journal und führe { $solstone_link_join } erneut aus, oder schreib an { $support_solstone_app } und wir helfen.{ $line_break }

journal.cli.sol.link.mark.join.unverifiable = die Überprüfung ist nicht gelungen.{ $line_break }die mark des Journals konnte von diesem Computer nicht ermittelt werden, die Wörter, die du angegeben hast, lassen sich also nicht vergleichen. dieser Computer ist nicht gekoppelt, und nichts, was er behalten hat, ging über diesen Link an jenes Journal. hol einen frischen Kopplungs-Link von deinem Journal und führe { $solstone_link_join } in einem Terminal aus, um selbst zu entscheiden.{ $line_break }

journal.cli.sol.link.mark.join.wrong = nicht gekoppelt.{ $line_break }die Wörter der mark, die du angegeben hast, stimmen nicht mit der mark des Journals überein, dieser Computer ist also nicht gekoppelt, und nichts, was er behalten hat, ging über diesen Link an jenes Journal. prüfe die Wörter, hol einen frischen Kopplungs-Link von deinem Journal und führe { $solstone_link_join } erneut aus.{ $line_break }

journal.cli.sol.link.mark.prompt.identified = noch ein Schritt: prüfe die mark deines Journals.{ $line_break }{ $line_break }{ $space_2 }die mark deines Journals: { $spoken }{ $line_break }{ $line_break }dein Journal zeigt dieselbe mark in seiner Netzwerk-App. sie sollte genau übereinstimmen.{ $line_break }stimmt das mit deinem Journal überein? gib { $yes } oder { $no } ein:{ $line_break }

journal.cli.sol.link.mark.prompt.unavailable = noch ein Schritt: prüfe die mark deines Journals.{ $line_break }{ $line_break }{ $space_2 }die mark deines Journals: gerade nicht verfügbar{ $line_break }{ $line_break }die Überprüfung ist nicht gelungen.{ $line_break }die mark deines Journals konnte von diesem Computer nicht ermittelt werden, es gibt also nichts zu vergleichen. { $continue } nur, wenn du sicher bist, dass der Link von deinem Journal gekommen ist.{ $line_break }gib { $continue } ein, um trotzdem zu koppeln, oder { $cancel }, um anzuhalten:{ $line_break }

journal.cli.sol.link.mark.terminal.required = { $solstone_link_join } kann dich hier nicht nach der mark deines Journals fragen. führe es in einem Terminal aus oder gib die zwei Wörter der mark mit { $mark_word_word } an.{ $line_break }

journal.cli.sol.link.mark.usage = gib die zwei Wörter der mark deines Journals an, wie { $mark_bramble_quokka }.{ $line_break }

journal.cli.sol.link.pair.closed = Kopplung fehlgeschlagen: das Kopplungsfenster ist geschlossen oder abgelaufen. Erzeuge einen neuen Kopplungscode und versuch es erneut.

journal.cli.sol.link.pair.enrollment = Relay hat die Geräteregistrierung abgelehnt ({ $endpoint } { $http } { $status }).

journal.cli.sol.link.pair.failed = Kopplung fehlgeschlagen ({ $code }). Erzeuge einen neuen Kopplungscode und versuch es erneut.

journal.cli.sol.link.pair.listener = Es konnte keine Verbindung zum Kopplungs-Listener hergestellt werden.

journal.cli.sol.link.pair.rejected = Kopplung fehlgeschlagen ({ $http } { $status }): das Kopplungsfenster ist geschlossen oder der Code wurde bereits verwendet.

journal.cli.sol.link.relay.abnormal = Die Relay-Verbindung wurde abnormal geschlossen. Versuch es erneut oder nutze { $direct } auf { $lan_vpn }.

journal.cli.sol.link.relay.closed = Das Relay-Kopplungsfenster ist geschlossen. Führe { $solstone_link_join } mit einem frischen Code erneut aus.

journal.cli.sol.link.relay.offline = das Relay meldet, dass dein Journal offline ist. starte das Journal oder nutze { $direct } auf lan/vpn.

journal.cli.sol.link.relay.overflow = Das Relay ist vorübergehend überlastet. Versuch es erneut oder nutze { $direct } auf { $lan_vpn }.

journal.cli.sol.link.relay.stalled = Die Relay-Verbindung ist stehen geblieben. Versuch es erneut oder nutze { $direct } auf { $lan_vpn }.

journal.cli.sol.link.relay.unauthorized = das Relay hat dieses Geräte-Token abgelehnt. führe { $solstone_link_join } für dieses Gerät erneut aus.

journal.cli.sol.link.relay.unknown = das Relay kennt dieses Journal nicht. führe { $solstone_link_join } erneut aus.

journal.cli.sol.link.relay.unpaid = Das Relay-Konto ist nicht verfügbar. Prüfe den Status von Relay-Dienst/Konto oder nutze { $direct }.

journal.cli.sol.link.relay.upgrade = Das Relay hat das WebSocket-Upgrade abgelehnt. Prüfe { $relay_url } und versuch es erneut.

journal.cli.sol.link.response.attestation = Kopplungsantwort ohne { $home_attestation }

journal.cli.sol.link.response.cert = Kopplungsantwort ohne { $client_cert }

journal.cli.sol.link.response.chain = Kopplungsantwort ohne { $ca_chain }

journal.cli.sol.link.response.endpoints = Die Kopplungsantwort { $local_endpoints } muss ein Array sein.

journal.cli.sol.link.response.instance = Kopplungsantwort ohne { $instance_id }

journal.cli.sol.link.response.key = Kopplungsantwort ohne erzeugten Client-Schlüssel

journal.cli.sol.link.serve.bind.failed = kann { $value_127_0_0_1 }{ $port } nicht binden: Binden fehlgeschlagen

journal.cli.sol.link.serve.bind.in.use = kann { $value_127_0_0_1 }{ $port } nicht binden: Adresse wird bereits verwendet. Ein anderes `{ $solstone_link_serve }` oder Convey verwendet diesen Port möglicherweise bereits.

journal.cli.sol.link.serve.bridge.failed = Die Einrichtung der nativen Link-Brücke ist vor dem Bereitstellen fehlgeschlagen. Versuch es nach dem erneuten Installieren von { $solstone_core } erneut.

journal.cli.sol.link.serve.credentials.invalid = Die Link-Zugangsdaten sind ungültig. Führe { $solstone_link_join } vor { $solstone_link_serve } aus.

journal.cli.sol.link.serve.forward.direct = leite { $value_127_0_0_1 }{ $port } -> { $mark } über eine Direktverbindung weiter{ $line_break }

journal.cli.sol.link.serve.forward.direct.unnamed = leite { $value_127_0_0_1 }{ $port } -> dein Journal über eine Direktverbindung weiter{ $line_break }

journal.cli.sol.link.serve.forward.either = leite { $value_127_0_0_1 }{ $port } -> { $mark } über Direktverbindung oder Relay weiter{ $line_break }

journal.cli.sol.link.serve.forward.either.unnamed = leite { $value_127_0_0_1 }{ $port } -> dein Journal über Direktverbindung oder Relay weiter{ $line_break }

journal.cli.sol.link.serve.forward.relay = leite { $value_127_0_0_1 }{ $port } -> { $mark } nur über Relay weiter{ $line_break }

journal.cli.sol.link.serve.forward.relay.unnamed = leite { $value_127_0_0_1 }{ $port } -> dein Journal nur über Relay weiter{ $line_break }

journal.cli.sol.link.serve.help = Verwendung: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $line_break }Optionen:{ $line_break }{ $h_help }{ $space_12 }diese Hilfe anzeigen und beenden{ $line_break }{ $label_label }{ $space_9 }Link-Bundle-Label{ $line_break }{ $port_port }{ $space_11 }Loopback-Port, auf dem bereitgestellt wird (Standard: 5015){ $line_break }{ $relay_url_relay_url }{ $line_break }{ $space_24 }Die Relay-{ $url } überschreiben{ $line_break }{ $direct }{ $space_14 }Nur direkt: erreich das Journal über eine direkte{ $line_break }{ $space_24 }Verbindung, nie das Relay. nutze das, wenn dein Journal{ $line_break }{ $space_24 }direkt erreichbar ist (gleiches Netzwerk/vpn), um die Relay-{ $line_break }{ $space_24 }Abhängigkeit zu vermeiden.{ $line_break }{ $relay_only }{ $space_10 }Nur Relay: erreich das Journal über das Relay,{ $line_break }{ $space_24 }nie über eine Direktverbindung. nutze das, wenn direkte Verbindungen{ $line_break }{ $space_24 }nicht versucht werden dürfen, auch wenn dein Journal{ $line_break }{ $space_24 }lokal erreichbar ist.{ $line_break }

journal.cli.sol.link.serve.mark.unnamed = dieses Link-Bundle hat keine Journal-ID, es kann also nicht benannt werden. führe { $solstone_link_join } erneut aus.{ $line_break }

journal.cli.sol.link.serve.policy.conflict = Argument { $direct }: nicht erlaubt mit Argument { $relay_only }

journal.cli.sol.link.serve.port.invalid = Argument { $port }: ungültiger int-Wert: '{ $value }'

journal.cli.sol.link.serve.port.range = { $port } muss zwischen 1 und 65535 liegen

journal.cli.sol.link.serve.runtime.unavailable = Die native Link-Laufzeit ist nicht verfügbar. Installiere { $solstone_core } neu und versuch es erneut.

journal.cli.sol.link.serve.unavailable = Die Link-Bereitstellungsnaht ist nicht verfügbar.{ $line_break }

journal.cli.sol.link.serve.usage.error = Verwendung: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $solstone_link_serve }: Fehler: { $error }{ $line_break }

journal.cli.sol.link.serve.warning = Warnung: Entwicklungsnutzung nur auf vertrauenswürdigen Systemen.{ $line_break }diese Brücke gibt lokalen Aufrufern deine volle Journal-{ $api }-Befugnis.{ $line_break }führe sie nur dort aus, wo du aller Software vertraust, die auf diesem System läuft.

journal.cli.sol.link.sighting.certainty = { $line_break }{ $line_break }das ist weniger gewiss als eine Direktverbindung.

journal.cli.sol.link.sighting.claim.unshown = was geantwortet hat:{ $space_2 }die behauptete Identität konnte nicht gezeigt werden (behauptet, nicht überprüft)

journal.cli.sol.link.sighting.claimed = was geantwortet hat:{ $space_2 }{ $spoken } (behauptet, nicht überprüft)

journal.cli.sol.link.sighting.direct = unbekanntes Journal bei { $addr } gesehen:

journal.cli.sol.link.sighting.journal = dein Journal:{ $space_3 }{ $spoken }

journal.cli.sol.link.sighting.no.identity = was geantwortet hat:{ $space_2 }keine Identität vorgelegt

journal.cli.sol.link.sighting.relay = unbekanntes Journal über das Relay gesehen:

journal.cli.sol.link.sighting.reset = { $line_break }{ $line_break }wenn du dein Journal kürzlich zurückgesetzt hast, koppeln dieses Gerät erneut.

journal.cli.sol.link.status.error = { $solstone_link_status }: Fehler: { $error }{ $line_break }

journal.cli.sol.link.status.help = Verwendung: { $solstone_link_status_h_label_l }{ $line_break }{ $line_break }Link-Status und die beobachtete Version des entfernten Journals anzeigen.{ $line_break }{ $line_break }Optionen:{ $line_break }{ $h_help }{ $space_5 }diese Hilfe anzeigen und beenden{ $line_break }{ $label_label }{ $space_2 }Link-Bundle-Label (standardmäßig der einzige gekoppelte Link){ $line_break }

journal.cli.sol.link.status.label = Label: { $label }{ $line_break }

journal.cli.sol.link.status.live = Status: { $state }{ $line_break }

journal.cli.sol.link.status.persist.warning = Warnung: Verbindungsänderungen wurden möglicherweise nicht gespeichert. behalte deine Zugangsdaten; der vorherige Zustand liegt möglicherweise noch auf dem Datenträger.{ $line_break }

journal.cli.sol.link.status.stopped = Status: gestoppt{ $line_break }

journal.cli.sol.link.status.usage.error = Verwendung: { $solstone_link_status_h_label_l }{ $line_break }{ $solstone_link_status }: Fehler: { $error }{ $line_break }

journal.cli.sol.link.status.version.fresh = Journal-Version: { $version }{ $line_break }

journal.cli.sol.link.status.version.last = Journal-Version: { $version } (zuletzt bekannt){ $line_break }

journal.cli.sol.link.status.version.unknown = Journal-Version: unbekannt{ $line_break }

journal.cli.sol.link.transport.credential = das Material der Link-Zugangsdaten ist ungültig. führe { $solstone_link_join } für dieses Gerät erneut aus.

journal.cli.sol.link.transport.enroll.rejected = Die Relay-Registrierung wurde mit { $http } { $status } abgelehnt. Führe { $solstone_link_join } erneut aus, wenn die Bundle-Attestierung veraltet ist.

journal.cli.sol.link.transport.enrollment = Link-Zugangsdaten oder Relay-Registrierung fehlgeschlagen. Führe { $solstone_link_join } erneut aus, wenn ein erneuter Versuch es nicht behebt.

journal.cli.sol.link.transport.framing = Das { $spl }-Stream-Framing ist beim Bereitstellen fehlgeschlagen. Versuch es erneut; koppeln erneut, wenn es weitergeht.

journal.cli.sol.link.transport.http = Die Antwort des Journals über den Link konnte nicht geparst werden. Aktualisiere beide Peers oder versuch es erneut.

journal.cli.sol.link.transport.io = Die Ein-/Ausgabe des Link-Transports ist beim Bereitstellen fehlgeschlagen. Prüfe, ob das Journal über { $lan_vpn } oder Relay erreichbar ist, und versuch es dann erneut.

journal.cli.sol.link.transport.json = Relay- oder Brücken-{ $json } konnte nicht geparst werden. Prüfe die Relay-{ $url } und versuch es erneut.

journal.cli.sol.link.transport.no.endpoint = Keine Adresse des Journals ist verfügbar. Führe { $solstone_link_join } erneut aus oder übergib { $relay_url }, außer { $direct } wird absichtlich genutzt.

journal.cli.sol.link.transport.not.paired = Link-Zugangsdaten fehlen. Führe { $solstone_link_join } vor { $solstone_link_serve } aus.

journal.cli.sol.link.transport.offset = Die lokale Offset-Abfrage ist fehlgeschlagen. Prüfe die Systemuhr und versuch es erneut.

journal.cli.sol.link.transport.pair.data = gespeicherte Kopplungsdaten sind ungültig. führe { $solstone_link_join } für dieses Gerät erneut aus.

journal.cli.sol.link.transport.refresh.rejected = die Erneuerung des Relay-Tokens wurde mit { $http } { $status } abgelehnt. führe { $solstone_link_join } für dieses Gerät erneut aus.

journal.cli.sol.link.transport.rejected = Das gekoppelte Journal hat die Link-Anfrage mit { $http } { $status } abgelehnt.

journal.cli.sol.link.transport.tls = Der sichere Link-Handshake ist fehlgeschlagen. Führe { $solstone_link_join } erneut aus, wenn sich das Zertifikat des Journals oder die Kopplung geändert hat.

journal.cli.sol.link.unpaired.instructions = Dieses Gerät kann sich mit seiner gespeicherten Kopplung nicht mit deinem Journal verbinden. Um es erneut zu koppeln, beende '{ $solstone_link_serve }', entferne den Ordner { $path }, führe '{ $solstone_link_join_code_new_pa }' aus und starte dann '{ $solstone_link_serve }' erneut.{ $line_break }

journal.cli.sol.moved = '{ $command }' wurde nach '{ $journal_command } { $command }' verschoben; führe das stattdessen aus.{ $line_break }(Befehle, die das Journal auf diesem Computer ausführen, stehen unter '{ $journal_command }'; siehe '{ $journal_help }'.)

journal.cli.sol.moved.navigate = Verschoben nach `{ $solstone_journal_navigate }` — führe das stattdessen aus.{ $line_break }

journal.cli.sol.profile.cadence = Rhythmus:

journal.cli.sol.profile.description = Beschreibung: { $value }

journal.cli.sol.profile.detached = losgelöste Facetten: { $detached_label }

journal.cli.sol.profile.entity.id = entity_id: { $value }

journal.cli.sol.profile.full.interactions = { $space_2 }recent_interactions_count_30d: { $value }

journal.cli.sol.profile.full.interval = { $space_2 }avg_interval_days: { $value }

journal.cli.sol.profile.full.last.seen = { $space_2 }last_seen: { $value }

journal.cli.sol.profile.full.quiet = { $space_2 }gone_quiet_since: { $value }

journal.cli.sol.profile.identity = { $name } · { $kind } · facets={ $facets } · self={ $self_value } · hidden={ $hidden }

journal.cli.sol.profile.interactions = recent_interactions_count_30d: { $value }

journal.cli.sol.profile.interval = avg_interval_days: { $value }

journal.cli.sol.profile.last.seen = last_seen: { $value }

journal.cli.sol.profile.name = Name: { $value }

journal.cli.sol.profile.not.found = Profil nicht gefunden: { $name }

journal.cli.sol.profile.quiet = gone_quiet_since: { $value }

journal.cli.sol.profile.type = Typ: { $value }

journal.cli.sol.resident.platform = Signale zum Herunterfahren sind auf dieser Plattform nicht verfügbar

journal.cli.sol.resident.signal = die Einrichtung des Signals für den nativen { $solstone_name }-Resident ist fehlgeschlagen: { $error }

journal.cli.sol.resident.startup = das Leeren beim Start des nativen { $solstone_name }-Resident ist fehlgeschlagen: { $error }

journal.cli.sol.root.inspect = die Auflösung der Projektwurzel von { $solstone_name } (nativ) ist fehlgeschlagen: die aktuelle ausführbare Datei konnte nicht geprüft werden: { $error }

journal.cli.sol.root.package = die Auflösung der Projektwurzel von { $solstone_name } (nativ) ist fehlgeschlagen: der Quell-Checkout oder das installierte { $solstone_name }-Paket konnte von { $path } aus nicht gefunden werden

journal.cli.sol.skills.agent.invalid = { $skill_command }: Fehler: { $agent_flag } { $agent } wird nicht unterstützt

journal.cli.sol.skills.agent.required = { $skill_command }: Fehler: { $agent_flag } erfordert einen Wert

journal.cli.sol.skills.argument.invalid = { $skill_command }: Fehler: unerkanntes Argument { $argument }

journal.cli.sol.skills.bundle.linked = der gebündelte Skill enthält verknüpfte oder spezielle Einträge

journal.cli.sol.skills.cleanup.failed = eigene Skills auffrischen: { $path } installiert; der alte Skill unter { $backup } ließ sich nicht aufräumen: { $error }

journal.cli.sol.skills.config.decode = Dekodierfehler der Journal-Konfiguration: { $kind }

journal.cli.sol.skills.config.not.directory = { $path } ist kein Verzeichnis

journal.cli.sol.skills.copy.special = spezielle Dateien sind in eigenen Skill-Bäumen nicht erlaubt

journal.cli.sol.skills.copy.symlink = Symlinks sind in eigenen Skill-Bäumen nicht erlaubt

journal.cli.sol.skills.cwd.failed = { $solstone }: natives Skills-Arbeitsverzeichnis ist nicht verfügbar: { $error }. Führ diesen Befehl in einem zugänglichen Verzeichnis aus.

journal.cli.sol.skills.exe.current = die aktuelle Programmdatei konnte nicht bestimmt werden: { $error }

journal.cli.sol.skills.exe.parent = die aktuelle Programmdatei hat kein übergeordnetes Verzeichnis

journal.cli.sol.skills.file.utf = { $skill_file } ist als { $encoding } ungültig: { $error }

journal.cli.sol.skills.home.absolute = das Home muss ein absoluter Pfad sein

journal.cli.sol.skills.home.missing = { $solstone }: das Home der nativen Skills ist nicht verfügbar. Setz { $home } auf dein Home-Verzeichnis.

journal.cli.sol.skills.identity.census = der Zensus konnte nicht gelesen werden: { $detail }

journal.cli.sol.skills.identity.home.must.be.absolute = das Home muss absolut sein

journal.cli.sol.skills.identity.installation.identity.is.unsupported.for.android = die Installationsidentität wird für Android nicht unterstützt

journal.cli.sol.skills.identity.io.clone.namespace.directory = Namensraum-Verzeichnis klonen: { $error }

journal.cli.sol.skills.identity.io.clone.namespace.registry = Namensraum-Register klonen: { $error }

journal.cli.sol.skills.identity.io.open.home.directory = Home-Verzeichnis öffnen: { $error }

journal.cli.sol.skills.identity.io.open.namespace.directory = Namensraum-Verzeichnis öffnen: { $error }

journal.cli.sol.skills.identity.io.open.namespace.registry = Namensraum-Register öffnen: { $error }

journal.cli.sol.skills.identity.io.read.namespace.registry = Namensraum-Register lesen: { $error }

journal.cli.sol.skills.identity.io.stat.storage.directory = stat Speicherverzeichnis: { $error }

journal.cli.sol.skills.identity.io.stat.storage.file = stat Speicherdatei: { $error }

journal.cli.sol.skills.identity.namespace.entry.is.not.lowercase.hex = der Namensraum-Eintrag ist kein Hex in Kleinbuchstaben

journal.cli.sol.skills.identity.namespace.registry.entries.limit = das Namensraum-Register überschreitet 1024 Einträge

journal.cli.sol.skills.identity.namespace.registry.size.limit = das Namensraum-Register überschreitet 16 MiB

journal.cli.sol.skills.identity.path.escapes.its.absolute.root = der Pfad verlässt seine absolute Wurzel

journal.cli.sol.skills.identity.path.must.be.absolute.and.nul.free = der Pfad muss absolut und NUL-frei sein

journal.cli.sol.skills.identity.root = das Wurzel-Token konnte nicht aus { $path } abgeleitet werden: { $detail }

journal.cli.sol.skills.identity.storage.directory.does.not.have.private.mode = das Speicherverzeichnis hat nicht den Modus { $mode }

journal.cli.sol.skills.identity.storage.directory.is.not.owned.by.the.current.user = das Speicherverzeichnis gehört dir nicht

journal.cli.sol.skills.identity.storage.file.does.not.have.private.mode = die Speicherdatei hat nicht den Modus { $mode }

journal.cli.sol.skills.identity.storage.file.is.not.owned.by.the.current.user = die Speicherdatei gehört dir nicht

journal.cli.sol.skills.install.done = installiert { $agent } { $skill } -> { $path }

journal.cli.sol.skills.journal.absolute = das ausgewählte Journal muss ein absoluter Pfad sein

journal.cli.sol.skills.journal.resolved = der aufgelöste Journal-Pfad ist nicht absolut

journal.cli.sol.skills.operation.error = Fehler: { $operation } { $path }: { $detail }

journal.cli.sol.skills.owner.base = die Owner-Basis konnte nicht unter { $path } gebildet werden: { $detail }

journal.cli.sol.skills.path.absolute = der Pfad muss absolut sein

journal.cli.sol.skills.path.ancestor.absent = kein vorhandener Vorfahre für { $path } gefunden

journal.cli.sol.skills.path.canonical = der Vorfahre { $path } konnte nicht kanonisiert werden: { $error }

journal.cli.sol.skills.path.inspect = der Vorfahre { $path } konnte nicht inspiziert werden: { $error }

journal.cli.sol.skills.path.overlap = der Pfad { $path } überlappt das geschützte Journal

journal.cli.sol.skills.path.resolved.overlap = der aufgelöste Vorfahrenpfad { $path } überlappt das geschützte Journal

journal.cli.sol.skills.payload.failed = { $solstone }: die Payload-Wurzel der nativen Skills ist nicht verfügbar: { $detail }. Installier { $solstone } und { $core } neu.

journal.cli.sol.skills.payload.project = Fehler: Projekt-Skill unter { $path } erwartet

journal.cli.sol.skills.payload.umbrella = Fehler: gebündelter Dach-Skill unter { $skill_file } ({ $path }) erwartet

journal.cli.sol.skills.project.agent = Fehler: { $agent_flag } { $agent } wird mit { $project_flag } nicht unterstützt; verwende { $all_option } oder { $claude_option }

journal.cli.sol.skills.project.missing = Projekt-Skill unter { $path } erwartet

journal.cli.sol.skills.publish.rollback = Skill-Veröffentlichung fehlgeschlagen: { $error }; Wiederherstellen fehlgeschlagen: { $restore }; Sicherungspfad: { $path }

journal.cli.sol.skills.registry.unknown = das Installations-Register ist unbekannt

journal.cli.sol.skills.remove.directory = Entfernen verweigert: kein Verzeichnis

journal.cli.sol.skills.remove.done = entfernt { $agent } { $skill } -> { $path }

journal.cli.sol.skills.remove.retired = entfernt { $agent } { $skill } (ausgemusterter Produkt-Skill) -> { $path }

journal.cli.sol.skills.remove.stale = entfernt { $agent } { $skill } (veraltet) -> { $path }

journal.cli.sol.skills.remove.symlink = Entfernen verweigert: kein Symlink

journal.cli.sol.skills.replace.done = ersetzt { $agent } { $skill } -> { $path }

journal.cli.sol.skills.skip.absent = übersprungen { $agent } { $skill } (nichts zu entfernen)

journal.cli.sol.skills.skip.all = keine Konfigurationsverzeichnisse für KI-Programmier-Agenten gefunden — Skill-Registrierung übersprungen

journal.cli.sol.skills.skip.config = übersprungen { $agent } (Konfigurationsverzeichnis fehlt unter { $path })

journal.cli.sol.skills.status.absent = { $agent } { $skill } nicht installiert

journal.cli.sol.skills.status.header = Agent      Skill                Status

journal.cli.sol.skills.status.installed = { $agent } { $skill } installiert

journal.cli.sol.skills.subcommand.invalid = { $skill_command }: Fehler: unbekannter Unterbefehl { $command_name }

journal.cli.sol.skills.subcommand.missing = { $skill_command }: Fehler: fehlender Unterbefehl

journal.cli.sol.skills.symlink.unsupported = Symlinks anlegen wird nicht unterstützt

journal.cli.sol.skills.target.ineligible = das Ziel ist für die Installation nicht zulässig

journal.cli.sol.skills.target.parent = das Ziel hat kein übergeordnetes Verzeichnis

journal.cli.sol.skills.target.preserved = der Zielpfad überlappt das geschützte Journal oder ist bewahrt

journal.cli.sol.skills.tree.directory = der Skill-Baum braucht ein gewöhnliches Verzeichnis

journal.cli.sol.skills.tree.linked = verknüpfte Einträge sind in eigenen Skill-Bäumen nicht erlaubt

journal.cli.sol.skills.tree.reference = die Referenz des gebündelten Skills braucht ein Verzeichnis mit regulärer Datei { $skill_file }

journal.cli.sol.skills.tree.special = spezielle Einträge sind in eigenen Skill-Bäumen nicht erlaubt

journal.cli.sol.skills.usage = Verwendung: { $syntax }

journal.cli.sol.skills.warning.existing = Warnung { $agent } { $skill } -> { $path } (vorhandener Inhalt am Ziel bewahrt)

journal.cli.sol.skills.warning.retired = Warnung { $agent } { $skill } -> { $path } (entspricht nicht dem ausgemusterten Skill { $retired_skill }; vorhandener Inhalt unter { $path } bewahrt)

journal.cli.sol.skills.warning.stale = Warnung { $agent } { $skill } -> { $path } (vorhandener Inhalt am veralteten Ziel bewahrt)

journal.cli.sol.skills.warnings = Warnungen:

journal.cli.sol.speakers.attribute.segment.accumulated.voiceprints = { $line_break }Angesammelte Stimmabdrücke:

journal.cli.sol.speakers.attribute.segment.by.method = { $line_break }Nach Methode:

journal.cli.sol.speakers.attribute.segment.embeddings = { $quantity ->
    [one] { $spacing_2 }{ $entity_id }: { $embeddings } Einbettung
   *[other] { $spacing_2 }{ $entity_id }: { $embeddings } Einbettungen
    }

journal.cli.sol.speakers.attribute.segment.resolved = Aufgelöst:{ $spacing_2 }{ $resolved }

journal.cli.sol.speakers.attribute.segment.sentences = Sätze: { $count }

journal.cli.sol.speakers.attribute.segment.unmatched = Nicht zugeordnet: { $count }

journal.cli.sol.speakers.attribute.segment.wrote = { $line_break }Geschrieben: { $path }

journal.cli.sol.speakers.backfill.completed.count = Abgeschlossene Anzahl:{ $spacing_11 }{ $count }

journal.cli.sol.speakers.backfill.elapsed.s = Verstrichen:{ $spacing_19 }{ $elapsed }s

journal.cli.sol.speakers.backfill.error.count = Fehleranzahl:{ $spacing_15 }{ $count }

journal.cli.sol.speakers.backfill.errors.and.more = { $spacing_2 }... und { $count } weitere

journal.cli.sol.speakers.backfill.failure.detail = Fehlerdetail:{ $spacing_12 }{ $detail }

journal.cli.sol.speakers.backfill.failure.stage = Fehlerphase:{ $spacing_13 }{ $stage }

journal.cli.sol.speakers.backfill.last.seen.entities.seen = Gesehene Entitäten:{ $spacing_12 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.pending.by.entity = { $line_break }Ausstehend je Entität:

journal.cli.sol.speakers.backfill.last.seen.rows.pending = Ausstehende Zeilen:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.rows.written = Geschriebene Zeilen:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.speaker.label.files.read = Gelesene Sprecher-Label-Dateien: { $count }

journal.cli.sol.speakers.backfill.last.seen.voiceprint.rows.scanned = Gescannte Stimmabdruck-Zeilen:{ $spacing_2 }{ $count }

journal.cli.sol.speakers.backfill.note.backfill.operation.has.immutable = Hinweis: Nachfüllungsvorgang { $id } hat unveränderliche Flags.{ $line_break }Um mit kompatiblen Einstellungen fortzusetzen, versuch:{ $line_break }{ $spacing_2 }{ $command } { $option_operation_id } { $id } { $option_commit }{ $line_break }

journal.cli.sol.speakers.backfill.operation.id = Vorgangs-ID:{ $spacing_14 }{ $op_id }

journal.cli.sol.speakers.backfill.pending.count = Ausstehende Anzahl:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.protected.skipped = Geschützt (übersprungen):{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.selected.for.backfill = Für die Nachfüllung ausgewählt:{ $spacing_5 }{ $count }

journal.cli.sol.speakers.backfill.starting.speaker.attribution.backfill = Nachfüllung der Sprecherzuordnung wird gestartet...

journal.cli.sol.speakers.backfill.status = Status:{ $spacing_20 }{ $status }

journal.cli.sol.speakers.backfill.total.planned.count = Geplante Gesamtanzahl:{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.total.segments.scanned = Insgesamt gescannte Segmente:{ $spacing_4 }{ $count }

journal.cli.sol.speakers.bootstrap.bootstrapping.voiceprints.from.single.speaker = Bootstrap von Stimmabdrücken aus Einzelsprecher-Segmenten...

journal.cli.sol.speakers.bootstrap.stats.embeddings.saved = Gespeicherte Einbettungen: { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.duplicate = Übersprungene Einbettungen (Duplikat): { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.owner = Übersprungene Einbettungen (eigene): { $count }

journal.cli.sol.speakers.bootstrap.stats.entities.created = Erstellte Entitäten: { $count }

journal.cli.sol.speakers.bootstrap.stats.segments.scanned = Gescannte Segmente: { $count }

journal.cli.sol.speakers.bootstrap.stats.single.speaker.segments = Einzelsprecher-Segmente: { $count }

journal.cli.sol.speakers.bootstrap.stats.top.speakers.by.embedding.count = Top-Sprecher nach Anzahl der Einbettungen:

journal.cli.sol.speakers.bootstrap.stats.unique.speakers = Eindeutige Sprecher: { $count }

journal.cli.sol.speakers.build.from.tags.can.build.from.tags = Aufbau aus Tags möglich: { $available }

journal.cli.sol.speakers.build.from.tags.guidance = Anleitung: { $guidance }

journal.cli.sol.speakers.build.from.tags.manual.tags = Manuelle Tags: { $tags }

journal.cli.sol.speakers.build.from.tags.next.step = Nächster Schritt: { $step }

journal.cli.sol.speakers.build.from.tags.observed = Beobachtet: { $observed }

journal.cli.sol.speakers.build.from.tags.owner.centroid.already.confirmed = Dein Zentroid ist bereits bestätigt.

journal.cli.sol.speakers.build.from.tags.owner.centroid.confirmed.from.manual = Dein Zentroid ist aus manuellen Tags bestätigt (principal: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.build.from.tags.owner.manual.tags.are.not = Deine manuellen Tags sind nicht bereit.

journal.cli.sol.speakers.build.from.tags.reason = Grund: { $reason }

journal.cli.sol.speakers.build.from.tags.threshold = Schwelle: { $threshold }

journal.cli.sol.speakers.confirm.owner.review.error.owner.voice.candidates.must = Fehler: Deine Stimmkandidaten müssen im Browser bestätigt werden.{ $line_break }Prüfe und bestätige unter { $review_entry }

journal.cli.sol.speakers.correct.already.correct = Bereits korrekt: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.correct.corrected = Korrigiert { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }: { $old } -> { $new_speaker }

journal.cli.sol.speakers.correct.preview.with.solstone.call.speakers = Vorschau mit: { $command } { $old_speaker } { $new_speaker }

journal.cli.sol.speakers.correct.propagation.available.statements.in.segments = { $quantity ->
    [one] { $segments_quantity ->
        [one] Weitergabe verfügbar: { $statements } Aussage in { $segments } Segment würde sich ändern
       *[other] Weitergabe verfügbar: { $statements } Aussage in { $segments } Segmenten würde sich ändern
    }
   *[other] { $segments_quantity ->
        [one] Weitergabe verfügbar: { $statements } Aussagen in { $segments } Segment würden sich ändern
       *[other] Weitergabe verfügbar: { $statements } Aussagen in { $segments } Segmenten würden sich ändern
    }
    }

journal.cli.sol.speakers.correct.propagation.nothing.else.would.change = Weitergabe: sonst würde sich nichts ändern

journal.cli.sol.speakers.correct.unassigned.unassigned = nicht zugewiesen

journal.cli.sol.speakers.correct.voiceprint.removal = Entfernen des Stimmabdrucks: { $outcome }

journal.cli.sol.speakers.day.segments.showing.of.segments.limit = { $quantity ->
    [one] Zeige { $shown } von { $total } Segment (Limit { $limit })
   *[other] Zeige { $shown } von { $total } Segmenten (Limit { $limit })
    }

journal.cli.sol.speakers.discover.cluster.samples.across.segments = { $quantity ->
    [one] { $segments_quantity ->
        [one] { $spacing_2 }Cluster { $cluster }: { $samples } Probe über { $segments } Segment
       *[other] { $spacing_2 }Cluster { $cluster }: { $samples } Probe über { $segments } Segmente
    }
   *[other] { $segments_quantity ->
        [one] { $spacing_2 }Cluster { $cluster }: { $samples } Proben über { $segments } Segment
       *[other] { $spacing_2 }Cluster { $cluster }: { $samples } Proben über { $segments } Segmente
    }
    }

journal.cli.sol.speakers.discover.found.unknown.speaker.cluster.s = { $quantity ->
    [one] { $clusters } unbekannter Sprecher-Cluster gefunden:{ $line_break }
   *[other] { $clusters } unbekannte Sprecher-Cluster gefunden:{ $line_break }
    }

journal.cli.sol.speakers.discover.no.recurring.unknown.speakers.found = Keine wiederkehrenden unbekannten Sprecher gefunden.

journal.cli.sol.speakers.discover.sid = { $spacing_4 }- { $day }/{ $stream }/{ $segment } sid={ $sentence_id }: { $text }

journal.cli.sol.speakers.discovery.error.try.again = versuch es noch einmal

journal.cli.sol.speakers.discovery.warnings.warning = Warnung: { $reason_code }: { $message }

journal.cli.sol.speakers.discovery.warnings.warning.count = Warnung: { $reason_code }: { $message } count={ $count }

journal.cli.sol.speakers.errors.errors = { $line_break }Fehler ({ $count }):

journal.cli.sol.speakers.follow.backfill.backfill.in.progress.status.completed = Nachfüllung läuft: status={ $status_kind }, completed={ $completed }/{ $total }, pending={ $pending }

journal.cli.sol.speakers.follow.backfill.backfill.not.completed.status.to = { $line_break }Nachfüllung nicht abgeschlossen (Status: { $status_kind }).{ $line_break }Zum Prüfen des Status:{ $line_break }{ $spacing_2 }{ $command_1 } { $operation_id }{ $line_break }Zum Fortsetzen:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.follow.repair.start.repair.in.progress.segments.repaired = Reparatur läuft, bisher reparierte Segmente: { $count }

journal.cli.sol.speakers.follow.repair.start.repair.running = Reparatur läuft: { $operation_id }

journal.cli.sol.speakers.identify.error.inspect.operations.with.solstone.call = Vorgänge ansehen mit: { $command }

journal.cli.sol.speakers.identify.error.inspect.this.operation.with.solstone = Diesen Vorgang ansehen mit: { $command } { $operation_id }

journal.cli.sol.speakers.identify.error.retry.with.the.same.request = Versuch es erneut mit { $option_request_id } { $request_id }, unverändert.

journal.cli.sol.speakers.identify.name.required.usage = Verwendung: { $command } { $syntax }{ $line_break }Versuch '{ $help_command }' für Hilfe.{ $line_break }{ $box_left } Fehler { $box_top }{ $line_break }{ $box_edge } Ungültiger Wert: name oder { $entity_option } ist erforderlich{ $box_padding }{ $box_edge }{ $line_break }{ $box_bottom }{ $line_break }

journal.cli.sol.speakers.parse.stream.layout.option.error.invalid.value.for.stream = Fehler: ungültiger Wert für { $option_stream_layout }

journal.cli.sol.speakers.presence.candidate.meeting.unvoiced.screen.meeting.days.voice.no = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.meeting.voiced.screen.meeting.days.voice.yes = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.candidate.mention.unvoiced.setting.speakers.voice.no = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.mention.voiced.setting.speakers.voice.yes = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.cluster.statements.segments.conversations = { $quantity ->
    [one] { $segments_quantity ->
        [one] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } Aussage, { $segments } Segment, { $conversations } Gespräch
           *[other] Cluster { $cluster }: { $statements } Aussage, { $segments } Segment, { $conversations } Gespräche
        }
       *[other] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } Aussage, { $segments } Segmente, { $conversations } Gespräch
           *[other] Cluster { $cluster }: { $statements } Aussage, { $segments } Segmente, { $conversations } Gespräche
        }
    }
   *[other] { $segments_quantity ->
        [one] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } Aussagen, { $segments } Segment, { $conversations } Gespräch
           *[other] Cluster { $cluster }: { $statements } Aussagen, { $segments } Segment, { $conversations } Gespräche
        }
       *[other] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } Aussagen, { $segments } Segmente, { $conversations } Gespräch
           *[other] Cluster { $cluster }: { $statements } Aussagen, { $segments } Segmente, { $conversations } Gespräche
        }
    }
    }

journal.cli.sol.speakers.presence.cluster.was.not.found.run = Cluster { $cluster_id } wurde nicht gefunden.{ $line_break }Führe '{ $command }' aus, um gültige Cluster-IDs zu erzeugen.{ $line_break }

journal.cli.sol.speakers.presence.co.presence = { $line_break }Ko-Präsenz:

journal.cli.sol.speakers.presence.evidence.complete = Nachweis: vollständig

journal.cli.sol.speakers.presence.evidence.gap.s = { $quantity ->
    [one] Nachweis: { $gaps } Lücke
   *[other] Nachweis: { $gaps } Lücken
    }

journal.cli.sol.speakers.presence.mentions = { $line_break }Erwähnungen:

journal.cli.sol.speakers.presence.no.candidate.entities.found = Keine Kandidaten-Entitäten gefunden.

journal.cli.sol.speakers.propagate.applied.applied.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
        [one] Angewendet: { $statement_count } Aussage in { $segment_count } Segment
       *[other] Angewendet: { $statement_count } Aussage in { $segment_count } Segmenten
    }
   *[other] { $segment_count_quantity ->
        [one] Angewendet: { $statement_count } Aussagen in { $segment_count } Segment
       *[other] Angewendet: { $statement_count } Aussagen in { $segment_count } Segmenten
    }
    }

journal.cli.sol.speakers.propagate.errors = { $line_break }Fehler:

journal.cli.sol.speakers.propagate.nothing.else.would.change = Sonst würde sich nichts ändern.

journal.cli.sol.speakers.propagate.reverse.with.solstone.call.commit = Rückgängig mit: { $command } { $operation } { $old_speaker } { $new_speaker } { $option_commit }

journal.cli.sol.speakers.propagate.would.change.would.change.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
        [one] Würde sich ändern: { $statement_count } Aussage in { $segment_count } Segment
       *[other] Würde sich ändern: { $statement_count } Aussage in { $segment_count } Segmenten
    }
   *[other] { $segment_count_quantity ->
        [one] Würden sich ändern: { $statement_count } Aussagen in { $segment_count } Segment
       *[other] Würden sich ändern: { $statement_count } Aussagen in { $segment_count } Segmenten
    }
    }

journal.cli.sol.speakers.rebuild.owner.guidance = Anleitung: { $step }

journal.cli.sol.speakers.rebuild.owner.next.step = Nächster Schritt: { $next_step }

journal.cli.sol.speakers.rebuild.owner.override.applied.true = Überschreibung angewendet: wahr

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuild.did.not = Der Neuaufbau deines Zentroids hat nicht geschrieben.

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuilt.principal.cluster = Dein Zentroid wurde neu aufgebaut (principal: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.rebuild.owner.owner.centroid.unchanged.evidence.already = Dein Zentroid ist unverändert; der Nachweis stimmt bereits überein.

journal.cli.sol.speakers.reject.owner.error.version.is.required = Fehler: { $option_version } ist erforderlich.

journal.cli.sol.speakers.repair.check.status.advice.to.check.status.solstone.call = { $line_break }Zum Prüfen des Status:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.dry.run.complete = Abgeschlossen:{ $spacing_2 }{ $complete }

journal.cli.sol.speakers.repair.dry.run.entities.with.voiceprints.to.remove = { $line_break }Entitäten mit zu entfernenden Stimmabdrücken: { $entities }

journal.cli.sol.speakers.repair.dry.run.gaps.detected = { $line_break }Erkannte Lücken ({ $gaps }):

journal.cli.sol.speakers.repair.dry.run.mode.dry.run = Modus:{ $spacing_6 }Probelauf

journal.cli.sol.speakers.repair.dry.run.rows = { $quantity ->
    [one] { $spacing_2 }- { $id } ({ $rows } Zeile)
   *[other] { $spacing_2 }- { $id } ({ $rows } Zeilen)
    }

journal.cli.sol.speakers.repair.dry.run.segments.to.repair = { $line_break }Zu reparierende Segmente: { $segments }

journal.cli.sol.speakers.repair.in.progress.status.advice = { $line_break }Die Reparatur läuft möglicherweise noch im Journal. Starte keine weitere.{ $line_break }Zum Prüfen des Status:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.starting.speaker.contamination.repair = Reparatur der Sprecher-Verunreinigung wird gestartet...

journal.cli.sol.speakers.repair.status.clean = Sauber:{ $spacing_5 }{ $clean }

journal.cli.sol.speakers.repair.status.last.failure = Letzter Fehler: { $stage } ({ $code })

journal.cli.sol.speakers.repair.status.segments.repaired = Reparierte Segmente: { $count }

journal.cli.sol.speakers.repair.status.status = Status:{ $spacing_4 }{ $status }

journal.cli.sol.speakers.repair.stopped.advice.to.resume.solstone.call.speakers = { $line_break }Zum Fortsetzen:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.report.only = NUR BERICHT — übergib { $option_commit }, um zu persistieren.{ $line_break }

journal.cli.sol.speakers.resolve.names.ambiguous = { $line_break }Mehrdeutig ({ $count }):

journal.cli.sol.speakers.resolve.names.auto.merged = { $line_break }Automatisch zusammengeführt ({ $count }):

journal.cli.sol.speakers.resolve.names.entities.with.voiceprints = Entitäten mit Stimmabdrücken: { $count }

journal.cli.sol.speakers.resolve.names.high.similarity.pairs = Paare mit hoher Ähnlichkeit: { $count }

journal.cli.sol.speakers.resolve.names.merging.name.variants.can.t = Das Zusammenführen von Namensvarianten lässt sich nicht rückgängig machen. Prüfe den Bericht ohne { $option_commit }, und führe ihn, sobald du genehmigst, erneut mit { $option_commit } { $option_yes } aus.

journal.cli.sol.speakers.resolve.names.pairs.compared = Verglichene Paare: { $count }

journal.cli.sol.speakers.resolve.names.resolving.speaker.name.variants = Namensvarianten von Sprechern werden aufgelöst...

journal.cli.sol.speakers.seed.from.imports.seeding.voiceprints.from.import.segments = Seeding von Stimmabdrücken aus Import-Segmenten...

journal.cli.sol.speakers.seed.from.imports.segments.with.speakers = Segmente mit Sprechern: { $count }

journal.cli.sol.speakers.seed.from.imports.speakers.by.embedding.count = Sprecher nach Anzahl der Einbettungen:

journal.cli.sol.speakers.seed.from.imports.unmatched.speakers = { $line_break }Nicht zugeordnete Sprecher ({ $count }):

journal.cli.sol.speakers.sentences.sentences.for = Sätze für { $day }/{ $stream }/{ $segment }/{ $source }:

journal.cli.sol.speakers.tag.owner.owner.sentence.already.tagged = Dein Satz ist bereits getaggt: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.tag.owner.tagged.owner.sentence = Getaggter Satz von dir: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.wipe.couldn.t.erase.now.it = konnte jetzt nicht endgültig gelöscht werden; es wartet im Papierkorb

journal.cli.sol.speakers.wipe.erase.needs.commit = { $option_erase } braucht { $option_commit }

journal.cli.sol.speakers.wipe.erased.erased = endgültig gelöscht

journal.cli.sol.speakers.wipe.files.b = { $quantity ->
    [one] { $label }: { $files } Datei ({ $bytes } B)
   *[other] { $label }: { $files } Dateien ({ $bytes } B)
    }

journal.cli.sol.speakers.wipe.in.the.trash = im Papierkorb

journal.cli.sol.speakers.wipe.in.the.trash.erases = im Papierkorb; löscht { $day } endgültig

journal.cli.sol.speakers.wipe.total.files.b = { $quantity ->
    [one] gesamt{ $spacing_14 }: { $files } Datei ({ $bytes } B)
   *[other] gesamt{ $spacing_14 }: { $files } Dateien ({ $bytes } B)
    }

journal.cli.sol.unsupported = Nicht unterstützter nativer { $solstone_name }-Befehl.

journal.cli.sol.update.automatic.choice = deine Auswahl konnte nicht gespeichert werden.

journal.cli.sol.update.automatic.home = dein Datenverzeichnis konnte nicht gefunden werden.

journal.cli.sol.update.automatic.ineligible = diese Installation kann sich nicht selbst aktualisieren. automatische Aktualisierungen funktionieren bei einer Installation von { $install_url }.

journal.cli.sol.update.automatic.ineligible.generic = diese Installation kann sich nicht selbst aktualisieren.

journal.cli.sol.update.automatic.linux = { $automatic_command } funktioniert unter Linux.

journal.cli.sol.update.automatic.off = automatische Aktualisierungen sind aus. zum Aktualisieren führe { $update_command } aus.

journal.cli.sol.update.automatic.on = automatische Aktualisierungen sind an. dein Journal startet nie von selbst neu; eine neue Version startet beim nächsten Neustart.

journal.cli.sol.update.automatic.package.add = aus einem Paket installiert. für Aktualisierungen über den Updater deines Systems füg das { $brand }-Repository hinzu: { $package_url }

journal.cli.sol.update.automatic.prefix = der Name des Installationsverzeichnisses passt nicht in eine Dienstdatei.

journal.cli.sol.update.automatic.run = { $result ->
    [up-to-date] automatische Aktualisierung: auf dem neuesten Stand
    [installed] automatische Aktualisierung: eine neue Version installiert
    [off] automatische Aktualisierung: automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierung: angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierung: Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierung: der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierung: der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierung: wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierung: wartet auf das nächste Release
    [busy] automatische Aktualisierung: eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierung: diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierung: die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.session = die tägliche Aktualisierungsprüfung ließ sich nicht einrichten. deine systemd-Benutzersitzung läuft vielleicht nicht.

journal.cli.sol.update.automatic.status = { $choice ->
    [on] automatische Aktualisierungen: an
    [off] automatische Aktualisierungen: aus
    [unasked] automatische Aktualisierungen: nicht eingeschaltet
    *[paused] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben
    }

journal.cli.sol.update.automatic.status.checked = { $choice ->
    [on] automatische Aktualisierungen: an · zuletzt geprüft { $date }
    [off] automatische Aktualisierungen: aus · zuletzt geprüft { $date }
    [unasked] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date }
    *[paused] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date }
    }

journal.cli.sol.update.automatic.status.off.result = { $result ->
    [up-to-date] automatische Aktualisierungen: aus · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: aus · eine neue Version installiert
    [off] automatische Aktualisierungen: aus · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: aus · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: aus · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: aus · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: aus · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: aus · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: aus · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: aus · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: aus · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: aus · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.status.off.result.checked = { $result ->
    [up-to-date] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · eine neue Version installiert
    [off] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: aus · zuletzt geprüft { $date } · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.status.on.result = { $result ->
    [up-to-date] automatische Aktualisierungen: an · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: an · eine neue Version installiert
    [off] automatische Aktualisierungen: an · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: an · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: an · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: an · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: an · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: an · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: an · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: an · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: an · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: an · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.status.on.result.checked = { $result ->
    [up-to-date] automatische Aktualisierungen: an · zuletzt geprüft { $date } · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: an · zuletzt geprüft { $date } · eine neue Version installiert
    [off] automatische Aktualisierungen: an · zuletzt geprüft { $date } · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: an · zuletzt geprüft { $date } · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: an · zuletzt geprüft { $date } · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: an · zuletzt geprüft { $date } · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: an · zuletzt geprüft { $date } · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: an · zuletzt geprüft { $date } · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: an · zuletzt geprüft { $date } · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: an · zuletzt geprüft { $date } · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: an · zuletzt geprüft { $date } · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: an · zuletzt geprüft { $date } · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.status.paused.result = { $result ->
    [up-to-date] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · eine neue Version installiert
    [off] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.status.paused.result.checked = { $result ->
    [up-to-date] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · eine neue Version installiert
    [off] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: angehalten nach zwei Versuchen, die nicht funktioniert haben · zuletzt geprüft { $date } · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.status.unasked.result = { $result ->
    [up-to-date] automatische Aktualisierungen: nicht eingeschaltet · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: nicht eingeschaltet · eine neue Version installiert
    [off] automatische Aktualisierungen: nicht eingeschaltet · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: nicht eingeschaltet · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: nicht eingeschaltet · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: nicht eingeschaltet · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: nicht eingeschaltet · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: nicht eingeschaltet · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: nicht eingeschaltet · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: nicht eingeschaltet · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: nicht eingeschaltet · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: nicht eingeschaltet · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.automatic.status.unasked.result.checked = { $result ->
    [up-to-date] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · auf dem neuesten Stand
    [installed] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · eine neue Version installiert
    [off] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · automatische Aktualisierungen sind aus
    [paused] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · angehalten nach zwei Versuchen, die nicht funktioniert haben
    [held] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · Aktualisierungen sind zurückgehalten
    [hold-unknown] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · der Update-Halt kann nicht gelesen werden
    [offline] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · der Aktualisierungsdienst konnte nicht erreicht werden
    [metered] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · wartet auf eine Verbindung, die nicht getaktet ist
    [waiting-for-signed-installer] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · wartet auf das nächste Release
    [busy] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · eine andere Aktualisierung lief
    [not-eligible] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · diese Installation kann sich nicht selbst aktualisieren
    *[failed] automatische Aktualisierungen: nicht eingeschaltet · zuletzt geprüft { $date } · die Aktualisierung hat nicht funktioniert ({ $code })
    }

journal.cli.sol.update.catalogue.download = der Release-Katalog konnte nicht geholt werden.

journal.cli.sol.update.catalogue.key = der Release-Schlüssel in diesem Build ist unlesbar.

journal.cli.sol.update.catalogue.lane = Release-Lane unbekannt: { $lane }

journal.cli.sol.update.catalogue.latest = die neueste Release-Markierung ist fehlerhaft.

journal.cli.sol.update.catalogue.pin = der Release-Katalog nennt einen anderen Release-Schlüssel.

journal.cli.sol.update.catalogue.release = der Release-Katalog stimmt nicht mit dem angeforderten Release überein.

journal.cli.sol.update.catalogue.schema.bad.catalogue.revision = der Release-Katalog ist ungültig: fehlerhafte Katalogrevision.

journal.cli.sol.update.catalogue.schema.bad.desktop.version = der Release-Katalog ist ungültig: fehlerhafte Desktop-Version.

journal.cli.sol.update.catalogue.schema.bad.installer = der Release-Katalog ist ungültig: fehlerhaftes Installationsprogramm.

journal.cli.sol.update.catalogue.schema.bad.version = der Release-Katalog ist ungültig: fehlerhafte Version.

journal.cli.sol.update.catalogue.schema.installer.in.an.unsupported.schema = der Release-Katalog ist ungültig: Installationsprogramm in einem nicht unterstützten Schema.

journal.cli.sol.update.catalogue.schema.key.id = der Release-Katalog ist ungültig: Schlüssel-ID.

journal.cli.sol.update.catalogue.schema.no.journal.version = der Release-Katalog ist ungültig: keine Journal-Version.

journal.cli.sol.update.catalogue.schema.no.version = der Release-Katalog ist ungültig: keine Version.

journal.cli.sol.update.catalogue.schema.not.json = der Release-Katalog ist ungültig: kein json.

journal.cli.sol.update.catalogue.schema.unsupported.protocol = der Release-Katalog ist ungültig: nicht unterstütztes Protokoll.

journal.cli.sol.update.catalogue.schema.unsupported.schema = der Release-Katalog ist ungültig: nicht unterstütztes Schema.

journal.cli.sol.update.catalogue.signature = die Signaturprüfung des Release-Katalogs ist fehlgeschlagen.

journal.cli.sol.update.catalogue.version = die angeforderte Plattformversion ist ungültig.

journal.cli.sol.update.receipt.claimed.twice = Komponente { $component } von den Belegen tree und package beansprucht

journal.cli.sol.update.receipt.duplicate.roles = die Komponenten journal und cli sind beide aufgezeichnet

journal.cli.sol.update.receipt.incomplete = der Installationsbeleg ist unvollständig oder nicht unterstützt. nutz das ursprüngliche Installationsprogramm zur Wiederherstellung.

journal.cli.sol.update.receipt.key.duplicate = doppelter Schlüssel { $key }

journal.cli.sol.update.receipt.key.outside = Schlüssel außerhalb des Abschnitts

journal.cli.sol.update.receipt.line.invalid = fehlerhafte Zeile: { $line }

journal.cli.sol.update.receipt.line.pair = nicht key=value

journal.cli.sol.update.receipt.native.file = nativ { $receipt_file } ist keine reguläre Datei

journal.cli.sol.update.receipt.native.malformed = fehlerhaft nativ { $receipt_file }: { $detail }

journal.cli.sol.update.receipt.native.route = Nativ-Belegroute muss tree sein

journal.cli.sol.update.receipt.native.schema = nativer Beleg { $schema_field } muss 1 sein

journal.cli.sol.update.receipt.native.version = installierte Version stimmt nicht mit Beleg { $journal_field } überein

journal.cli.sol.update.receipt.package.file = Paket { $config_file } ist keine reguläre Datei

journal.cli.sol.update.receipt.package.malformed = fehlerhaftes Paket { $config_file }: { $detail }

journal.cli.sol.update.receipt.section.duplicate = doppelter Abschnitt [{ $section }]

journal.cli.sol.update.receipt.section.key.duplicate = doppelter Schlüssel { $key } im Abschnitt [{ $section }]

journal.cli.sol.update.receipt.section.unclosed = ungeschlossener Abschnittskopf

journal.cli.sol.update.receipt.section.unknown = nicht erkannter Abschnitt in { $config_file }: { $section }

journal.cli.sol.update.receipt.unrecorded = dieses Journal wurde aus einem Paket ohne das { $brand }-Installationsprogramm installiert, deshalb kann { $update_command } es noch nicht aktualisieren. führe { $install_command } einmal aus. dabei werden die installierten Dateien gegen das signierte Release geprüft, und danach aktualisiert { $update_command } es.

journal.cli.sol.update.receipt.user.file = Benutzerdatei { $config_file } ist keine reguläre Datei

journal.cli.sol.update.receipt.user.malformed = fehlerhafte Benutzerdatei { $config_file }: { $detail }

journal.cli.sol.update.report.available = eine Aktualisierung ist verfügbar.

journal.cli.sol.update.report.available.package.add = eine Aktualisierung ist verfügbar. aus einem Paket installiert. für Aktualisierungen über den Updater deines Systems füg das { $brand }-Repository hinzu: { $package_url }

journal.cli.sol.update.report.available.package.configured = eine Aktualisierung ist verfügbar. aus einem Paket installiert. Aktualisierungen kommen über den Updater deines Systems.

journal.cli.sol.update.report.catalogue.invalid = die ausgewählten Release-Metadaten sind ungültig.

journal.cli.sol.update.report.checkout = diese Kopie ist ein Quell-Checkout. installiere das veröffentlichte Journal, um es zu aktualisieren.

journal.cli.sol.update.report.current = dein Journal ist auf dem neuesten Stand.

journal.cli.sol.update.report.current.package.add = dein Journal ist auf dem neuesten Stand. aus einem Paket installiert. für Aktualisierungen über den Updater deines Systems füg das { $brand }-Repository hinzu: { $package_url }

journal.cli.sol.update.report.current.package.configured = dein Journal ist auf dem neuesten Stand. aus einem Paket installiert. Aktualisierungen kommen über den Updater deines Systems.

journal.cli.sol.update.report.held = Aktualisierungen sind zurückgehalten. führe { $unhold_command } aus, um sie freizugeben.

journal.cli.sol.update.report.hold.changed = der Update-Halt kann nicht gelesen werden. nichts wurde geändert.

journal.cli.sol.update.report.hold.off = Aktualisierungen sind nicht mehr zurückgehalten.

journal.cli.sol.update.report.hold.on = Aktualisierungen sind zurückgehalten.

journal.cli.sol.update.report.hold.save = der Update-Halt konnte nicht gespeichert werden. prüf die Berechtigungen deines Datenverzeichnisses.

journal.cli.sol.update.report.hold.unreadable = der Update-Halt kann nicht gelesen werden.

journal.cli.sol.update.report.install.directory = das Installationsverzeichnis konnte nicht ermittelt werden.

journal.cli.sol.update.report.installer.arguments = ungültige Argumente des Installationsprogramms.

journal.cli.sol.update.report.installer.failed = Installation ist nicht gelungen. nutz das ursprüngliche Installationsprogramm zur Wiederherstellung.

journal.cli.sol.update.report.installer.fetch = das Installationsprogramm konnte nicht geholt werden.

journal.cli.sol.update.report.installer.run = das Installationsprogramm konnte nicht ausgeführt werden.

journal.cli.sol.update.report.installer.tookover = dein Journal wurde aktualisiert. es aktualisiert jetzt mit dem { $brand }-Installationsprogramm, von dort, wo es ist.

journal.cli.sol.update.report.installer.updated = dein Journal wurde aktualisiert.

journal.cli.sol.update.report.linux = { $update_command } ist unter Linux verfügbar.

journal.cli.sol.update.report.mode = dieser Modus ist keine Aktualisierung.

journal.cli.sol.update.report.owner.unknown = wie diese Kopie von { $brand } installiert wurde, war nicht feststellbar. aktualisiere mit dem Installationsprogramm, das sie installiert hat.

journal.cli.sol.update.report.platform.older = dieser ältere Build konnte keinem Plattform-Release zugeordnet werden. wähl eine Plattformversion mit { $version_flag }.

journal.cli.sol.update.report.rollback.package = diese Paketroute kann kein älteres Journal installieren. nichts wurde geändert.

journal.cli.sol.update.report.unknown = Aktualisierungsstatus ist unbekannt.

journal.cli.sol.update.rollback.epoch = installiere die aktuelle Version oder ein anderes Release in dieser Upgrade-Epoche

journal.cli.sol.update.rollback.link = Link auf die aktuelle Version fehlt oder ist unlesbar

journal.cli.sol.update.rollback.metadata = installierte Release-Metadaten sind unlesbar

journal.cli.sol.update.rollback.no.older = kein geeigneter älterer Build ist behalten. wähl eine Plattformversion mit { $version_flag }, oder installiere das aktuelle Release neu.

journal.cli.sol.update.rollback.policy = das aktuelle Release hat keine unterstützte Aufbewahrungsrichtlinie

journal.cli.sol.update.rollback.range.absent = das installierte Release hat keinen deklarierten Leserbereich. nichts wurde geändert.

journal.cli.sol.update.rollback.range.outside = die ausgewählte Version liegt außerhalb des deklarierten Leserbereichs. nichts wurde geändert.

journal.cli.sol.update.rollback.retained = dieser Build ist nicht behalten; installiere die aktuelle Version neu

journal.cli.sol.update.rollback.state = das Installationsprogramm kann den Zustand deines Journals nicht feststellen. nichts wurde geändert.

journal.cli.sol.update.rollback.version = Zielversion { $target_version } ist nicht älter als { $installed_version }

journal.cli.sol.update.summary.failure = { $code }: { $message }

journal.cli.sol.update.summary.hold.held = Halt: zurückgehalten

journal.cli.sol.update.summary.hold.unheld = Halt: nicht zurückgehalten

journal.cli.sol.update.summary.hold.unknown = Halt: unbekannt

journal.cli.sol.update.summary.journal = journal: { $value }

journal.cli.sol.update.summary.lane = Lane: { $value }

journal.cli.sol.update.summary.platform = Plattform: { $value }

journal.cli.sol.update.summary.route = Route: { $value }

journal.cli.sol.update.summary.target = verfügbar: { $available } (journal { $journal })

journal.cli.sol.usage = Verwendung: { $syntax }
