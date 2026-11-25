journal.cli.journal.archive.argument = argomento imprevisto

journal.cli.journal.archive.day.exclusive = { $day_flag } non è combinabile con { $from_flag } o { $to_flag }

journal.cli.journal.archive.day.invalid = { $token_day } deve essere una data { $token_yyyymmdd } reale

journal.cli.journal.archive.day.required = { $day_flag } richiede { $token_yyyymmdd }

journal.cli.journal.archive.encode.finish.body = completamento del corpo dell'archivio compresso: { $detail }

journal.cli.journal.archive.encode.finish.finalize = finalizzazione del completamento dell'archivio compresso: { $detail }

journal.cli.journal.archive.encode.member = scrittura dell'archivio compresso { $member }: { $detail }

journal.cli.journal.archive.encode.metadata = metadati dell'archivio compresso non validi { $field } { $value }: { $reason }

journal.cli.journal.archive.encode.source = origine dell'archivio compresso: { $detail }

journal.cli.journal.archive.encode.write = scrittura dell'archivio compresso: { $detail }

journal.cli.journal.archive.encode.writer = writer dell'archivio compresso non valido: { $reason }

journal.cli.journal.archive.export.help = Uso: { $syntax }{ $line_break }{ $line_break }crea un { $token_zip } del tuo diario, incluse le impostazioni e le credenziali.{ $line_break }registri di elaborazione, indici ricostruibili e cache, lock e file temporanei sono esclusi.{ $line_break }con un filtro di data, sono inclusi solo i giorni selezionati.

journal.cli.journal.archive.exports.unsafe = directory delle esportazioni non sicura: { $path }

journal.cli.journal.archive.fact.archive.output.contains.a.null.byte = l'output dell'archivio compresso contiene un byte nullo

journal.cli.journal.archive.fact.archive.output.ends.with.a.separator = l'output dell'archivio compresso termina con un separatore

journal.cli.journal.archive.fact.archive.output.has.no.file.name = l'output dell'archivio compresso non ha un nome di file

journal.cli.journal.archive.fact.archive.output.has.no.parent = l'output dell'archivio non ha una directory superiore

journal.cli.journal.archive.fact.archive.output.must.be.one.file.name = l'output dell'archivio compresso deve essere un solo nome di file

journal.cli.journal.archive.fact.archive.output.parent.is.missing.or.unsafe = la directory superiore dell'output dell'archivio compresso manca o non è sicura

journal.cli.journal.archive.fact.archive.output.path.is.not.valid.utf8 = il percorso dell'output dell'archivio compresso non è un { $token_utf_8 } valido

journal.cli.journal.archive.fact.archive.output.uses.an.unsupported.prefix = l'output dell'archivio compresso usa un prefisso non supportato

journal.cli.journal.archive.fact.archive.parent.component.is.invalid = il componente superiore dell'archivio compresso non è valido

journal.cli.journal.archive.fact.block.device = dispositivo a blocchi

journal.cli.journal.archive.fact.character.device = dispositivo a caratteri

journal.cli.journal.archive.fact.could.not.determine.writer.position = non è stato possibile determinare la posizione del writer

journal.cli.journal.archive.fact.could.not.read.writer.flags = non è stato possibile leggere i flag del writer

journal.cli.journal.archive.fact.could.not.stat.writer = non è stato possibile eseguire stat del writer

journal.cli.journal.archive.fact.directory = directory

journal.cli.journal.archive.fact.fifo = fifo

journal.cli.journal.archive.fact.filesystem.root.is.not.a.directory = la radice del filesystem non è una directory

journal.cli.journal.archive.fact.has.no.parent.directory = non ha una directory superiore

journal.cli.journal.archive.fact.must.be.a.regular.file = deve essere un file regolare

journal.cli.journal.archive.fact.must.be.empty = deve essere vuoto

journal.cli.journal.archive.fact.must.be.opened.for.writing = deve essere aperto per la scrittura

journal.cli.journal.archive.fact.must.be.positioned.at.the.start = deve essere posizionato all'inizio

journal.cli.journal.archive.fact.must.be.valid.utf8 = deve essere un { $token_utf_8 } valido

journal.cli.journal.archive.fact.must.name.one.normal.file.leaf = deve nominare una sola foglia di file normale

journal.cli.journal.archive.fact.must.not.be.opened.in.append.mode = non deve essere aperto in modalità di accodamento

journal.cli.journal.archive.fact.must.resolve.to.an.absolute.path = deve risolversi in un percorso assoluto

journal.cli.journal.archive.fact.other = altro

journal.cli.journal.archive.fact.regular.file = file regolare

journal.cli.journal.archive.fact.relative.archive.output.requires.an.absolute.cwd = l'output relativo dell'archivio compresso richiede un cwd assoluto

journal.cli.journal.archive.fact.socket = socket

journal.cli.journal.archive.fact.symlink = collegamento simbolico

journal.cli.journal.archive.fact.utf8.member.path.exceeds.zip.u16.byte.limit = il percorso del membro { $token_utf_8 } supera il limite di byte { $token_u16 } di { $token_zip }

journal.cli.journal.archive.from.invalid = { $token_from } deve essere una data { $token_yyyymmdd } reale

journal.cli.journal.archive.from.required = { $from_flag } richiede { $token_yyyymmdd }

journal.cli.journal.archive.import.ambiguities = tipo non corrispondente in { $token_entities_ambiguities_jsonl }

journal.cli.journal.archive.import.appeared = comparsa di { $relative } mentre l'importazione era in esecuzione

journal.cli.journal.archive.import.busy = il lock di unione dell'archivio compresso è occupato in { $path }: { $detail }

journal.cli.journal.archive.import.cleanup = non è stato possibile pulire l'estrazione { $path }: { $detail }

journal.cli.journal.archive.import.compressed = { $count ->
    [one] l'archivio compresso { $path } è di { $bytes } byte; il massimo è { $maximum }
    [many] l'archivio compresso { $path } è di { $bytes } di byte; il massimo è { $maximum }
   *[other] l'archivio compresso { $path } è di { $bytes } byte; il massimo è { $maximum }
    }

journal.cli.journal.archive.import.coordinate = coordinata di registrazione pianificata non valida

journal.cli.journal.archive.import.directory.remains = la directory rimane

journal.cli.journal.archive.import.encrypted.extraction = elemento cifrato dell'archivio compresso durante l'estrazione: { $entry }

journal.cli.journal.archive.import.encrypted.validation = elemento cifrato dell'archivio compresso durante la validazione: { $entry }

journal.cli.journal.archive.import.entity = non è stato possibile unire l'entità { $entity_id }: { $detail }

journal.cli.journal.archive.import.expanded = { $count ->
    [one] l'archivio compresso si espande a { $bytes } byte; il massimo è { $maximum }
    [many] l'archivio compresso si espande a { $bytes } di byte; il massimo è { $maximum }
   *[other] l'archivio compresso si espande a { $bytes } byte; il massimo è { $maximum }
    }

journal.cli.journal.archive.import.extract = non è stato possibile estrarre { $archive } in { $dir }: { $detail }

journal.cli.journal.archive.import.facet = non è stato possibile unire la facetta { $facet }: { $detail }

journal.cli.journal.archive.import.handling = durante la gestione di { $path }: { $detail }

journal.cli.journal.archive.import.hidden.archive = i giorni nascosti di dati del corpo di questo archivio compresso non si possono leggere, quindi questo archivio compresso non si può importare

journal.cli.journal.archive.import.hidden.details = { $detail }; { $warning }

journal.cli.journal.archive.import.hidden.target = questo diario non può leggere quali giorni di dati del corpo hai nascosto, quindi questo archivio compresso non si può importare

journal.cli.journal.archive.import.hidden.warning = { $days ->
    [one] 1 giorno di dati del corpo era nascosto nell'archivio compresso, quindi è nascosto anche in questo diario; puoi mostrarlo di nuovo nella pagina del corpo.
    [many] { $days } di giorni di dati del corpo erano nascosti nell'archivio compresso, quindi sono nascosti anche in questo diario; puoi mostrarli di nuovo nella pagina del corpo.
   *[other] { $days } giorni di dati del corpo erano nascosti nell'archivio compresso, quindi sono nascosti anche in questo diario; puoi mostrarli di nuovo nella pagina del corpo.
    }

journal.cli.journal.archive.import.invalid = archivio compresso non valido { $path }: { $detail }

journal.cli.journal.archive.import.kind = tipo non corrispondente in { $relative }

journal.cli.journal.archive.import.legacy = un archivio compresso { $token_tgz } o { $token_tar_gz } non è supportato. se è un vecchio archivio compresso di trasferimento, esporta di nuovo quel giorno come { $token_zip } dal diario da cui proviene e uniscilo, oppure unisci questo file in un diario nuovo e vuoto su una versione { $token_2_x } precedente, esporta quel diario come { $token_zip }, e unisci quel { $token_zip } nel tuo diario

journal.cli.journal.archive.import.lock = il lock di unione dell'archivio compresso non è riuscito in { $path }: { $detail }

journal.cli.journal.archive.import.log = non è stato possibile scrivere il registro delle decisioni in { $path }: { $detail }

journal.cli.journal.archive.import.magic = magic dell'archivio compresso non riconosciuto

journal.cli.journal.archive.import.marker = non è stato possibile aggiornare il marcatore del flusso dopo la pubblicazione del contenuto dell'archivio compresso: { $detail }; il contenuto pubblicato non è stato ripristinato

journal.cli.journal.archive.import.marker.day = { $day }: { $detail }

journal.cli.journal.archive.import.merge = non è stato possibile unire l'importazione in { $path }: { $detail }

journal.cli.journal.archive.import.missing = archivio compresso non trovato: { $path }

journal.cli.journal.archive.import.name = nome di percorso non-{ $token_utf_8 } o mancante

journal.cli.journal.archive.import.owner = attendi il detentore oppure ispeziona/rimuovi solo i metadati del titolare verificati come non aggiornati in { $path }

journal.cli.journal.archive.import.path.encoding = { $path } non è { $token_utf_8 }

journal.cli.journal.archive.import.path.outside = { $path } è fuori dal diario

journal.cli.journal.archive.import.publish = non è stato possibile pubblicare l'unione dell'archivio compresso: { $detail }

journal.cli.journal.archive.import.redirect.entity = questo diario ha unito '{ $source_id }' in '{ $survivor }', e le unioni non si possono annullare, quindi questo archivio compresso non si può importare

journal.cli.journal.archive.import.redirect.facet = questo diario aveva una facetta di nome '{ $facet }' che è stata unita o rinominata in un'altra, e quel nome ora porta lì, quindi questo archivio compresso non si può importare

journal.cli.journal.archive.import.segment = non è stato possibile unire il segmento in { $path }: { $detail }

journal.cli.journal.archive.import.segment.appeared = il segmento è comparso prima della pubblicazione dell'archivio compresso

journal.cli.journal.archive.import.segment.changed = il segmento pubblicato dell'archivio compresso è cambiato

journal.cli.journal.archive.import.segment.symlink = collegamento simbolico nel segmento

journal.cli.journal.archive.import.space = spazio libero insufficiente in { $path }: { $available } disponibile, { $required } richiesto

journal.cli.journal.archive.import.stage = non è stato possibile scrivere la preparazione in { $path }: { $detail }

journal.cli.journal.archive.import.stage.changed = la directory superiore della preparazione dell'archivio compresso è cambiata

journal.cli.journal.archive.import.stopped = importazione fermata

journal.cli.journal.archive.import.symlink = link simbolico

journal.cli.journal.archive.import.traversal = percorso assoluto o di attraversamento

journal.cli.journal.archive.import.unambiguous = l'archivio compresso non ha una radice del diario univoca

journal.cli.journal.archive.import.undo = { $detail }; annullamento incompleto: { $failures }

journal.cli.journal.archive.import.undo.coordinate = coordinata { $relative }: { $detail }

journal.cli.journal.archive.import.undo.restore = ripristino di { $relative }: { $detail }

journal.cli.journal.archive.import.undo.rewrite = riscrittura della mappa delle identità: { $detail }

journal.cli.journal.archive.import.undo.unlink = unlink di { $relative }: { $detail }

journal.cli.journal.archive.import.unimplemented = fonti di importazione: non implementato: { $module }

journal.cli.journal.archive.import.unsafe.extraction = elemento non sicuro dell'archivio compresso durante l'estrazione: { $entry }: { $detail }

journal.cli.journal.archive.import.unsafe.validation = elemento non sicuro dell'archivio compresso durante la validazione: { $entry }: { $detail }

journal.cli.journal.archive.incomplete = { $committed } confermati, { $skipped } saltati, { $failed } falliti

journal.cli.journal.archive.ios = non disponibile su { $token_ios }

journal.cli.journal.archive.merge.help = Uso: { $syntax }

journal.cli.journal.archive.metadata.reason.invalid.calendar.date.or.time = data o ora di calendario non valida

journal.cli.journal.archive.metadata.reason.invalid.day = giorno non valido

journal.cli.journal.archive.metadata.reason.invalid.hour = ora non valida

journal.cli.journal.archive.metadata.reason.invalid.minute = minuto non valido

journal.cli.journal.archive.metadata.reason.invalid.month = mese non valido

journal.cli.journal.archive.metadata.reason.invalid.second = secondo non valido

journal.cli.journal.archive.metadata.reason.must.contain.digits.in.numeric.positions = deve contenere cifre nelle posizioni numeriche

journal.cli.journal.archive.metadata.reason.must.not.be.empty = non deve essere vuoto

journal.cli.journal.archive.metadata.reason.must.not.contain.ascii.control.characters = non deve contenere caratteri di controllo ASCII

journal.cli.journal.archive.metadata.reason.must.use.yyyy.mm.ddthh.mm.ssz = deve usare YYYY-MM-DDTHH:MM:SSZ

journal.cli.journal.archive.metadata.reason.second.must.be.in.seconds = il secondo deve essere in 0..=59

journal.cli.journal.archive.metadata.reason.year.must.be.in.years = l'anno deve essere in 1980..=2107

journal.cli.journal.archive.negative.identity = l'identità dell'origine è negativa

journal.cli.journal.archive.negative.size = la dimensione del file regolare è negativa

journal.cli.journal.archive.out.required = { $out_flag } richiede { $token_path }

journal.cli.journal.archive.output.parent = l'output dell'archivio non ha una directory superiore

journal.cli.journal.archive.output.tree = l'output è dentro l'albero esportato { $family }

journal.cli.journal.archive.preview = Unione dell'archivio compresso (giorni: { $days }): 0 confermati, { $skipped } saltati, 0 falliti.

journal.cli.journal.archive.publish.cleanup = pulizia del file temporaneo dell'archivio compresso: { $detail }

journal.cli.journal.archive.publish.createtemp = creazione del file temporaneo dell'archivio compresso: { $detail }

journal.cli.journal.archive.publish.publish = pubblicazione dell'archivio compresso: { $detail }

journal.cli.journal.archive.publish.syncdirectory = sincronizzazione della directory dell'archivio compresso: { $detail }

journal.cli.journal.archive.publish.synctemp = sincronizzazione del file temporaneo dell'archivio compresso: { $detail }

journal.cli.journal.archive.range = { $from_flag } non deve essere dopo { $to_flag }

journal.cli.journal.archive.root.name = la radice del diario non ha un nome di file

journal.cli.journal.archive.root.parent = la radice del diario non ha una directory superiore

journal.cli.journal.archive.scratch.name = non è stato possibile allocare un file di lavoro esclusivo dell'archivio compresso

journal.cli.journal.archive.skipped = { $token_solstone_journal_archive_export }: elementi di primo livello saltati: { $skipped }

journal.cli.journal.archive.source.budget = radice del diario non supportata { $path }: l'origine dell'archivio compresso supera il suo budget di inventario supportato

journal.cli.journal.archive.source.changed = l'origine del diario è cambiata in { $member }

journal.cli.journal.archive.source.io.list.journal.directory = elenco della directory del diario: { $detail }

journal.cli.journal.archive.source.io.list.journal.directory.member = elenco della directory del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.member = { $operation } { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing = apertura della directory del diario per l'elenco: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.for.listing.member = apertura della directory del diario per l'elenco { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.directory.member = apertura della directory del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.open.journal.file = apertura del file del diario: { $detail }

journal.cli.journal.archive.source.io.open.journal.file.member = apertura del file del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root = apertura della radice del diario conservato: { $detail }

journal.cli.journal.archive.source.io.open.retained.journal.root.member = apertura della radice del diario conservato { $member }: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file = lettura del file inventariato: { $detail }

journal.cli.journal.archive.source.io.read.inventoried.file.member = lettura del file inventariato { $member }: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size = lettura della dimensione del file regolare: { $detail }

journal.cli.journal.archive.source.io.read.regular.file.size.member = lettura della dimensione del file regolare { $member }: { $detail }

journal.cli.journal.archive.source.io.read.source.file.identity.member = lettura dell'identità del file di origine { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory = stat della directory inventariata: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.directory.member = stat della directory inventariata { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file = stat del file inventariato: { $detail }

journal.cli.journal.archive.source.io.stat.inventoried.file.member = stat del file inventariato { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry = stat dell'elemento del diario: { $detail }

journal.cli.journal.archive.source.io.stat.journal.entry.member = stat dell'elemento del diario { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory = stat della directory inventariata aperta: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.directory.member = stat della directory inventariata aperta { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file = stat del file inventariato aperto: { $detail }

journal.cli.journal.archive.source.io.stat.opened.inventoried.file.member = stat del file inventariato aperto { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory = stat della directory del diario aperta: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.directory.member = stat della directory del diario aperta { $member }: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file = stat del file del diario aperto: { $detail }

journal.cli.journal.archive.source.io.stat.opened.journal.file.member = stat del file del diario aperto { $member }: { $detail }

journal.cli.journal.archive.source.missing = { $token_source } non esiste: { $path }

journal.cli.journal.archive.source.regular = { $token_source } deve essere un file regolare

journal.cli.journal.archive.source.required = { $token_source } è obbligatorio

journal.cli.journal.archive.source.unsafe = elemento del diario non sicuro { $member }: { $kind }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness = attivazione del testimone ricorsivo dello spazio dei nomi: { $detail }

journal.cli.journal.archive.source.windows.arm.recursive.namespace.witness.member = attivazione del testimone ricorsivo dello spazio dei nomi { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root = prestito della radice ammessa del diario: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.member = prestito della radice ammessa del diario { $member }: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch = prestito del monitoraggio dello spazio dei nomi della radice ammessa del diario: { $detail }

journal.cli.journal.archive.source.windows.borrow.admitted.journal.root.namespace.watch.member = prestito del monitoraggio dello spazio dei nomi della radice ammessa del diario { $member }: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness = annullamento del testimone ricorsivo dello spazio dei nomi: { $detail }

journal.cli.journal.archive.source.windows.cancel.recursive.namespace.witness.member = annullamento del testimone ricorsivo dello spazio dei nomi { $member }: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness = controllo del testimone ricorsivo dello spazio dei nomi: { $detail }

journal.cli.journal.archive.source.windows.check.recursive.namespace.witness.member = controllo del testimone ricorsivo dello spazio dei nomi { $member }: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event = creazione dell'evento del testimone ricorsivo dello spazio dei nomi: { $detail }

journal.cli.journal.archive.source.windows.create.recursive.namespace.witness.event.member = creazione dell'evento del testimone ricorsivo dello spazio dei nomi { $member }: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness = svuotamento del testimone ricorsivo dello spazio dei nomi: { $detail }

journal.cli.journal.archive.source.windows.drain.recursive.namespace.witness.member = svuotamento del testimone ricorsivo dello spazio dei nomi { $member }: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory = elenco della directory conservata: { $detail }

journal.cli.journal.archive.source.windows.list.retained.directory.member = elenco della directory conservata { $member }: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry = apertura dell'elemento relativo conservato: { $detail }

journal.cli.journal.archive.source.windows.open.retained.relative.entry.member = apertura dell'elemento relativo conservato { $member }: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing = analisi dell'elenco della directory conservata: { $detail }

journal.cli.journal.archive.source.windows.parse.retained.directory.listing.member = analisi dell'elenco della directory conservata { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes = interrogazione degli attributi dell'elemento conservato: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.attributes.member = interrogazione degli attributi dell'elemento conservato { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity = interrogazione dell'identità dell'elemento conservato: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.identity.member = interrogazione dell'identità dell'elemento conservato { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata = interrogazione dei metadati dell'elemento conservato: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.metadata.member = interrogazione dei metadati dell'elemento conservato { $member }: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type = interrogazione del tipo dell'elemento conservato: { $detail }

journal.cli.journal.archive.source.windows.query.retained.entry.type.member = interrogazione del tipo dell'elemento conservato { $member }: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file = lettura del file osservato: { $detail }

journal.cli.journal.archive.source.windows.read.observed.file.member = lettura del file osservato { $member }: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity = riverifica dell'identità della directory relativa conservata: { $detail }

journal.cli.journal.archive.source.windows.reverify.retained.relative.directory.identity.member = riverifica dell'identità della directory relativa conservata { $member }: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer = calcolo della dimensione del buffer di lettura controllata: { $detail }

journal.cli.journal.archive.source.windows.size.checked.read.buffer.member = calcolo della dimensione del buffer di lettura controllata { $member }: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open = traccia dell'apertura dell'elenco della directory relativa conservata: { $detail }

journal.cli.journal.archive.source.windows.trace.retained.relative.directory.listing.open.member = traccia dell'apertura dell'elenco della directory relativa conservata { $member }: { $detail }

journal.cli.journal.archive.source.windows.unsupported.arm.recursive.namespace.witness = radice del diario non supportata { $path }: attivazione del testimone ricorsivo dello spazio dei nomi

journal.cli.journal.archive.source.windows.unsupported.cancel.recursive.namespace.witness = radice del diario non supportata { $path }: annullamento del testimone ricorsivo dello spazio dei nomi

journal.cli.journal.archive.source.windows.unsupported.check.recursive.namespace.witness = radice del diario non supportata { $path }: controllo del testimone ricorsivo dello spazio dei nomi

journal.cli.journal.archive.source.windows.unsupported.drain.recursive.namespace.witness = radice del diario non supportata { $path }: svuotamento del testimone ricorsivo dello spazio dei nomi

journal.cli.journal.archive.source.windows.unsupported.wait.for.recursive.namespace.witness = radice del diario non supportata { $path }: attesa del testimone ricorsivo dello spazio dei nomi

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness = attesa del testimone ricorsivo dello spazio dei nomi: { $detail }

journal.cli.journal.archive.source.windows.wait.for.recursive.namespace.witness.member = attesa del testimone ricorsivo dello spazio dei nomi { $member }: { $detail }

journal.cli.journal.archive.space.overflow = overflow dello spazio su disco disponibile

journal.cli.journal.archive.space.platform = l'ispezione dello spazio su disco non è supportata su questa piattaforma

journal.cli.journal.archive.summary = Unione dell'archivio: { $committed } confermati, { $skipped } saltati, { $failed } non riusciti.

journal.cli.journal.archive.target.changed = il percorso dell'output dell'archivio è cambiato durante l'esportazione: { $path }

journal.cli.journal.archive.target.collision = l'output dell'archivio esiste già: { $path }

journal.cli.journal.archive.target.invalid = output dell'archivio non valido { $path }: { $reason }

journal.cli.journal.archive.target.io = { $operation } non riuscito per { $path }: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.filesystem.root = apertura della radice del filesystem dell'output dell'archivio non riuscita per { $path }: { $detail }

journal.cli.journal.archive.target.io.open.archive.output.parent = apertura della directory superiore dell'output dell'archivio non riuscita per { $path }: { $detail }

journal.cli.journal.archive.target.io.retain.admitted.archive.output.parent = conservazione della directory superiore ammessa dell'output dell'archivio non riuscita per { $path }: { $detail }

journal.cli.journal.archive.target.io.revalidate.admitted.archive.output.parent = rivalidazione della directory superiore ammessa dell'output dell'archivio non riuscita per { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.admitted.archive.output.parent = stat della directory superiore ammessa dell'output dell'archivio non riuscito per { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.filesystem.root = stat della radice del filesystem dell'output dell'archivio non riuscito per { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.final.name = stat del nome finale dell'output dell'archivio non riuscito per { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.archive.output.parent = stat della directory superiore dell'output dell'archivio non riuscito per { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.opened.archive.output.parent = stat della directory superiore aperta dell'output dell'archivio non riuscito per { $path }: { $detail }

journal.cli.journal.archive.target.io.stat.retained.archive.output.parent = stat della directory superiore conservata dell'output dell'archivio non riuscito per { $path }: { $detail }

journal.cli.journal.archive.target.unsafe = l'output dell'archivio non è sicuro ({ $kind }): { $path }

journal.cli.journal.archive.target.windows.changed = l'output dell'archivio è cambiato: { $path }

journal.cli.journal.archive.target.windows.io = output dell'archivio { $path }: { $detail }

journal.cli.journal.archive.target.windows.unsafe = output dell'archivio non sicuro { $path }: { $reason }

journal.cli.journal.archive.temp.name = non è stato possibile allocare un nome temporaneo univoco per l'archivio

journal.cli.journal.archive.to.invalid = { $token_to } deve essere una data reale { $token_yyyymmdd }

journal.cli.journal.archive.to.required = { $to_flag } richiede { $token_yyyymmdd }

journal.cli.journal.archive.zero.write = il writer dell'archivio ha restituito zero byte per una scrittura non vuota

journal.cli.journal.facet.argument.ios = non disponibile su { $platform }

journal.cli.journal.facet.argument.unexpected = argomento imprevisto

journal.cli.journal.facet.content.unsafe = contenuto di facetta non sicuro: { $path }

journal.cli.journal.facet.doctor.adopted.heading = Facette orfane adottate

journal.cli.journal.facet.doctor.adoption.failed = { $destination } (adozione non riuscita: { $error })

journal.cli.journal.facet.doctor.adoption.unmerged = { $folders_remaining ->
    [one] { $destination } (adozione non riuscita: { $error }; { $unmerged } non è stata unita)
    [many] { $destination } (adozione non riuscita: { $error }; { $unmerged } non sono state unite)
   *[other] { $destination } (adozione non riuscita: { $error }; { $unmerged } non sono state unite)
}

journal.cli.journal.facet.doctor.argument.merge = { $merge_flag } richiede { $adopt_flag }

journal.cli.journal.facet.doctor.collision.heading = Collisioni di file regolari

journal.cli.journal.facet.doctor.empty = Niente da riportare.

journal.cli.journal.facet.doctor.failed.heading = Facette orfane non riuscite

journal.cli.journal.facet.doctor.fix.ios = { $fix_flag } non è disponibile su { $platform }

journal.cli.journal.facet.doctor.folders.hidden = Le cartelle i cui nomi iniziano con un punto non sono mai facette, quindi queste dichiarazioni di facetta vengono ignorate: { $folders }

journal.cli.journal.facet.doctor.group.ambiguous = { $members }: con un nome simile a { $facets }; rinomina la cartella in modo che corrisponda a una, poi esegui con { $adopt_flag } { $merge_flag }

journal.cli.journal.facet.doctor.group.damaged = { $members }: con un nome simile a { $facets }, il cui { $declaration } va riparato prima

journal.cli.journal.facet.doctor.help = Uso: { $syntax }{ $line_break }{ $retire_syntax }

journal.cli.journal.facet.doctor.history.failed = scansione del registro delle azioni non riuscita: { $error }

journal.cli.journal.facet.doctor.history.merged = { $name } è stata unita in { $destination }

journal.cli.journal.facet.doctor.history.recorded = Registrati i nomi di facette ritirate dai registri delle azioni:

journal.cli.journal.facet.doctor.history.renamed = { $name } è stata rinominata in { $destination }

journal.cli.journal.facet.doctor.history.reused = Queste facette sono state rinominate o unite in un'altra in precedenza e in seguito i loro nomi sono stati dati a una nuova facetta, quindi il materiale più vecchio che le nomina non si può distinguere. Non è stato cambiato niente: { $names }

journal.cli.journal.facet.doctor.history.unknown = I registri delle azioni citano questi nomi ma non dove sono finiti: { $names }. Se del materiale ne nomina ancora uno, registralo con { $retire_arguments } o { $deleted_flag }.

journal.cli.journal.facet.doctor.history.unrecorded = Nomi di facette che i registri delle azioni mostrano come ritirati ma che non sono registrati (esegui con { $fix_flag } per registrarli):

journal.cli.journal.facet.doctor.leftovers.failed = non è stato possibile leggere i residui dell'unione: { $error }

journal.cli.journal.facet.doctor.leftovers.kept = Cartelle residue di un'unione di facette che possono tenere l'unica copia di alcuni file. Non eliminarle; esegui con { $fix_flag } per spostarle nel cestino: { $folders }

journal.cli.journal.facet.doctor.leftovers.moved = Cartelle residue di un'unione di facette spostate nel cestino, così come erano: { $folders }

journal.cli.journal.facet.doctor.leftovers.safe = Cartelle residue di un'unione di facette interrotta, sicure da eliminare quando nessuna unione è in esecuzione: { $folders }

journal.cli.journal.facet.doctor.link.duplicate.open = { $facet }: '{ $id }' è collegata da più di una cartella ({ $folders })

journal.cli.journal.facet.doctor.link.duplicate.repaired = { $facet }: '{ $id }' era collegata da più di una cartella ({ $folders })

journal.cli.journal.facet.doctor.link.merged.open = { $folder_count ->
    [one] { $facet }: '{ $id }' è stata unita in '{ $successor }', e la cartella { $folders } la collega ancora
    [many] { $facet }: '{ $id }' è stata unita in '{ $successor }', e le cartelle { $folders } la collegano ancora
   *[other] { $facet }: '{ $id }' è stata unita in '{ $successor }', e le cartelle { $folders } la collegano ancora
}

journal.cli.journal.facet.doctor.link.merged.repaired = { $folder_count ->
    [one] { $facet }: '{ $id }' è stata unita in '{ $successor }', e la cartella { $folders } la collegava
    [many] { $facet }: '{ $id }' è stata unita in '{ $successor }', e le cartelle { $folders } la collegavano
   *[other] { $facet }: '{ $id }' è stata unita in '{ $successor }', e le cartelle { $folders } la collegavano
}

journal.cli.journal.facet.doctor.link.misnamed.open = { $facet }: '{ $id }' è collegata dalla cartella { $folders }, non da una cartella di nome '{ $id }'

journal.cli.journal.facet.doctor.link.misnamed.repaired = { $facet }: '{ $id }' era collegata dalla cartella { $folders }, non da una cartella di nome '{ $id }'

journal.cli.journal.facet.doctor.link.unreadable = { $facet }: la cartella { $folders } ha un collegamento che non si può leggere o usare

journal.cli.journal.facet.doctor.link.unreadable.check = { $facet }: la cartella { $folders } ha un collegamento che non si può leggere o usare; { $fix_flag } lo lascia così com'è

journal.cli.journal.facet.doctor.link.unreadable.undecided = { $facet }: la cartella { $folders } ha un collegamento che non si può leggere o usare ({ $fix_flag } non può capire quale entità collega, quindi lo lascia)

journal.cli.journal.facet.doctor.links.check = Collegamenti di entità da riparare (esegui con { $fix_flag } per riparare quelli che si possono):

journal.cli.journal.facet.doctor.links.check.failed = controllo dei collegamenti di entità non riuscito: { $error }

journal.cli.journal.facet.doctor.links.left = Collegamenti di entità lasciati così come sono:

journal.cli.journal.facet.doctor.links.repair.failed = riparazione dei collegamenti di entità non riuscita: { $error }

journal.cli.journal.facet.doctor.links.repaired = Collegamenti di entità riparati:

journal.cli.journal.facet.doctor.links.rescan = Esegui '{ $rescan_command }' per aggiornare l'indice.

journal.cli.journal.facet.doctor.lock.failed = il lock della facetta non è stato preso: { $error }

journal.cli.journal.facet.doctor.maintenance.heading = Errori di manutenzione di unioni confermate

journal.cli.journal.facet.doctor.merge.collision = { $source } -> { $destination }: { $path } (tenuto { $retained })

journal.cli.journal.facet.doctor.merge.failed = { $source } -> { $destination } (unione non riuscita prima della conferma: { $error })

journal.cli.journal.facet.doctor.merge.ios = unione di facette non disponibile su { $platform }

journal.cli.journal.facet.doctor.merged.heading = Facette orfane unite

journal.cli.journal.facet.doctor.merged.none = Nessuna facetta orfana è stata registrata o unita.

journal.cli.journal.facet.doctor.merges.incomplete = una o più unioni di facette orfane sono state confermate con errori di manutenzione

journal.cli.journal.facet.doctor.orphans.count = { $count ->
    [one] { $count } facetta orfana trovata. Esegui con { $adopt_flag } per registrarla.
    [many] { $count } di facette orfane trovate. Esegui con { $adopt_flag } per registrarle.
   *[other] { $count } facette orfane trovate. Esegui con { $adopt_flag } per registrarle.
}

journal.cli.journal.facet.doctor.orphans.heading = Facette orfane:

journal.cli.journal.facet.doctor.orphans.held = I registri delle azioni mostrano che questi nomi di facette sono stati ritirati, quindi le loro cartelle non sono registrate; esegui con { $fix_flag } per registrarli: { $folders }

journal.cli.journal.facet.doctor.orphans.none = Nessuna facetta orfana trovata.

journal.cli.journal.facet.doctor.orphans.retired = Queste cartelle hanno contenuto sotto nomi di facette che sono stati uniti o rinominati in un'altra facetta, quindi non sono registrate come facette; i loro file sono lasciati al loro posto: { $folders }

journal.cli.journal.facet.doctor.queue.failed = la riparazione è finita, ma non è stato possibile mettere in coda il suo aggiornamento della ricerca: { $error }

journal.cli.journal.facet.doctor.reference = - { $name }: { $count } (da { $first } a { $last })

journal.cli.journal.facet.doctor.references.command = Per renderne di nuovo raggiungibile una, esegui: { $retire_command }

journal.cli.journal.facet.doctor.references.failed = scansione dei riferimenti non riuscita: { $error }

journal.cli.journal.facet.doctor.references.missing = I segmenti nominano queste facette, che non esistono e non sono registrate come ritirate. Gli agenti limitati a facette scelte non possono vedere quel materiale; tutto il resto può:

journal.cli.journal.facet.doctor.registered.none = Nessuna facetta orfana è stata registrata.

journal.cli.journal.facet.doctor.registration.failed = registrata { $slug }, ma non è stato possibile scrivere il suo registro: { $error }

journal.cli.journal.facet.doctor.repair.failed = non è stato possibile riparare i nomi di facette ritirati: { $error }

journal.cli.journal.facet.doctor.repaired.count = { $repaired ->
    [one] { $repaired } facetta orfana riparata. la ricerca si sta aggiornando.
    [many] { $repaired } di facette orfane riparate. la ricerca si sta aggiornando.
   *[other] { $repaired } facette orfane riparate. la ricerca si sta aggiornando.
}

journal.cli.journal.facet.doctor.repaired.heading = Facette orfane riparate:

journal.cli.journal.facet.doctor.repairs.failed = una o più riparazioni di facette orfane non sono riuscite

journal.cli.journal.facet.doctor.repairs.failed.count = { $repaired ->
    [one] { $failed_orphans ->
        [one] { $repaired } facetta orfana riparata; { $failed_orphans } facetta orfana non riuscita. Esegui '{ $rescan_command }' per aggiornare l'indice.
        [many] { $repaired } facetta orfana riparata; { $failed_orphans } di facette orfane non riuscite. Esegui '{ $rescan_command }' per aggiornare l'indice.
       *[other] { $repaired } facetta orfana riparata; { $failed_orphans } facette orfane non riuscite. Esegui '{ $rescan_command }' per aggiornare l'indice.
        }
    [many] { $failed_orphans ->
        [one] { $repaired } di facette orfane riparate; { $failed_orphans } facetta orfana non riuscita. Esegui '{ $rescan_command }' per aggiornare l'indice.
        [many] { $repaired } di facette orfane riparate; { $failed_orphans } di facette orfane non riuscite. Esegui '{ $rescan_command }' per aggiornare l'indice.
       *[other] { $repaired } di facette orfane riparate; { $failed_orphans } facette orfane non riuscite. Esegui '{ $rescan_command }' per aggiornare l'indice.
        }
   *[other] { $failed_orphans ->
        [one] { $repaired } facette orfane riparate; { $failed_orphans } facetta orfana non riuscita. Esegui '{ $rescan_command }' per aggiornare l'indice.
        [many] { $repaired } facette orfane riparate; { $failed_orphans } di facette orfane non riuscite. Esegui '{ $rescan_command }' per aggiornare l'indice.
       *[other] { $repaired } facette orfane riparate; { $failed_orphans } facette orfane non riuscite. Esegui '{ $rescan_command }' per aggiornare l'indice.
        }
}

journal.cli.journal.facet.doctor.repairs.maintenance.count = { $repaired ->
    [one] { $merges ->
        [one] { $repaired } facetta orfana riparata; { $merges } unione confermata, ma ha segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
        [many] { $repaired } facetta orfana riparata; { $merges } di unioni confermate, ma hanno segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
       *[other] { $repaired } facetta orfana riparata; { $merges } unioni confermate, ma hanno segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
        }
    [many] { $merges ->
        [one] { $repaired } di facette orfane riparate; { $merges } unione confermata, ma ha segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
        [many] { $repaired } di facette orfane riparate; { $merges } di unioni confermate, ma hanno segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
       *[other] { $repaired } di facette orfane riparate; { $merges } unioni confermate, ma hanno segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
        }
   *[other] { $merges ->
        [one] { $repaired } facette orfane riparate; { $merges } unione confermata, ma ha segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
        [many] { $repaired } facette orfane riparate; { $merges } di unioni confermate, ma hanno segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
       *[other] { $repaired } facette orfane riparate; { $merges } unioni confermate, ma hanno segnalato un errore di manutenzione dopo la conferma. Vedi 'Errori di manutenzione di unioni confermate' sopra. Esegui '{ $rescan_command }' per aggiornare l'indice.
        }
}

journal.cli.journal.facet.doctor.retired.malformed = { $retired_path } non è analizzabile ({ $detail }). Esegui con { $fix_flag } per ricostruirlo; nessun nome che esso riserva viene liberato.

journal.cli.journal.facet.doctor.retired.rebuilt = { $retired_path } non era analizzabile ({ $detail }); è stato spostato in { $path } e ricostruito.

journal.cli.journal.facet.doctor.retired.reserved = Non è stato possibile rintracciare questi nomi e sono registrati come eliminati: { $names }.

journal.cli.journal.facet.doctor.retired.unreadable = non è stato possibile leggere { $retired_path } ({ $detail }). Controlla i suoi permessi; è stato lasciato così com'è.

journal.cli.journal.facet.doctor.search.failed = non è stato possibile mettere in coda l'aggiornamento della ricerca: { $error }

journal.cli.journal.facet.doctor.search.queued = la ricerca si sta aggiornando sulle modifiche delle facette.

journal.cli.journal.facet.doctor.search.skipped = La ricerca non è stata aggiornata, perché { $retired_path } non si può ancora usare.

journal.cli.journal.facet.doctor.twins.heading = Cartelle con un nome simile a una facetta che hai:

journal.cli.journal.facet.doctor.twins.merge = Queste non vengono mai registrate accanto a essa. Esegui con { $adopt_flag } { $merge_flag } per unirle in essa; un'unione è permanente e un file che hanno entrambe tiene la copia della facetta.

journal.cli.journal.facet.doctor.twins.preview = Per vedere prima che cosa porterebbe uno: { $merge_command } { $source } { $into_flag } { $destination } { $dry_run_flag }

journal.cli.journal.facet.doctor.unclear.heading = Cartelle lasciate così come sono

journal.cli.journal.facet.doctor.variants.heading = Gruppi di varianti del nome:

journal.cli.journal.facet.doctor.variants.merge = Queste non sono state registrate. Esegui con { $adopt_flag } { $merge_flag } per ridurle a una.

journal.cli.journal.facet.doctor.variants.register = Esegui con { $adopt_flag } { $merge_flag } per ridurre le varianti del nome prima di registrarle.

journal.cli.journal.facet.entry.unsafe = elemento di facetta non sicuro: { $path }

journal.cli.journal.facet.hidden.damaged = non è stato possibile leggere i nomi nascosti in { $path }, quindi non è stato cambiato niente: { $detail }

journal.cli.journal.facet.hidden.empty = non è stato dato nessun nome

journal.cli.journal.facet.hidden.field.text = { $key } non è testo

journal.cli.journal.facet.hidden.field.unknown = campo sconosciuto { $key }

journal.cli.journal.facet.hidden.names.list = { $names_field } non è una lista

journal.cli.journal.facet.hidden.row.empty = il nome è vuoto

journal.cli.journal.facet.hidden.row.error = nome { $index }: { $detail }

journal.cli.journal.facet.hidden.row.object = non è un oggetto

journal.cli.journal.facet.json.object = { $path } non è un oggetto { $json_format }

journal.cli.journal.facet.link.conflict = { $path } ha contenuti diversi nelle cartelle che vengono combinate, quindi non è stato combinato niente

journal.cli.journal.facet.link.file.encoding = { $path } non è testo { $encoding }

journal.cli.journal.facet.link.ids.exhausted = le note hanno esaurito i numeri di id

journal.cli.journal.facet.link.invalid.name = '{ $name }' non si può usare come id di entità

journal.cli.journal.facet.link.not.file = { $path } è una cartella o un link, non un file, quindi non è stato combinato niente

journal.cli.journal.facet.link.observation.corrupt = osservazione corrotta in { $source }: { $detail }

journal.cli.journal.facet.link.observation.malformed = osservazione malformata in { $source } alla riga { $line }: { $reason }

journal.cli.journal.facet.link.occupied = la cartella '{ $id }' tiene un'altra entità o non si può leggere

journal.cli.journal.facet.link.repair.destination = la cartella per '{ $id }' nella facetta verso cui si unisce tiene un'altra entità o non si può leggere, quindi non è stato cambiato niente; esegui '{ $doctor_command }' per vedere che cosa va riparato

journal.cli.journal.facet.link.repair.named = la cartella per '{ $id }' nella facetta '{ $facet }' tiene un'altra entità o non si può leggere, quindi non è stato cambiato niente; esegui '{ $doctor_command }' per vedere che cosa va riparato

journal.cli.journal.facet.link.successor.missing = non si trova l'entità a cui si è unita

journal.cli.journal.facet.log.action.queue = non è stato possibile mettere in coda l'aggiornamento della ricerca del registro delle azioni: { $error }

journal.cli.journal.facet.log.detections.queue = rilevamenti di facetta non in coda per la ricerca facet={ $facet } cause={ $error }

journal.cli.journal.facet.log.trash.unfinished = elemento del cestino { $id }: spostato, da finire più tardi: { $error }

journal.cli.journal.facet.merge.argument.confirm = un'unione è permanente e non si può annullare; rivedila con { $dry_run_flag }, poi eseguila di nuovo con { $yes_flag }

journal.cli.journal.facet.merge.argument.dest = { $into_flag } richiede { $destination_argument }

journal.cli.journal.facet.merge.argument.into = { $into_dest } è obbligatorio

journal.cli.journal.facet.merge.argument.invalid = non valido { $source_argument } o { $destination_argument }

journal.cli.journal.facet.merge.argument.source = { $source_argument } è obbligatorio

journal.cli.journal.facet.merge.cleanup.failed = l'unione è finita, ma non è stato possibile rimuovere le sue cartelle residue: { $error }; esegui '{ $repair_command }' per sistemarle

journal.cli.journal.facet.merge.collisions.kept = Entrambe le facette avevano questi file. '{ $destination }' ha tenuto la propria copia; la copia in '{ $source }' è stata tenuta, e '{ $repair_command }' la sposta nel cestino:

journal.cli.journal.facet.merge.collisions.trashed = Entrambe le facette avevano questi file. '{ $destination }' ha tenuto la propria copia; la copia in '{ $source }' è nel cestino:

journal.cli.journal.facet.merge.complete = Unita '{ $source }' in '{ $destination }'. Ricostruzione dell'indice completata.

journal.cli.journal.facet.merge.destination.absent = '{ $destination }' non è una facetta

journal.cli.journal.facet.merge.destination.id = '{ $destination }' non ha ancora un id stabile; esegui '{ $backfill_command }' prima

journal.cli.journal.facet.merge.destination.retired = '{ $destination }' è registrato come nome di facetta ritirato; esegui '{ $doctor_command }' per i dettagli

journal.cli.journal.facet.merge.detections.failed = i suoi rilevamenti non sono stati messi in coda

journal.cli.journal.facet.merge.failed = unione di facette non riuscita

journal.cli.journal.facet.merge.help = Uso: { $syntax }

journal.cli.journal.facet.merge.preview.empty = Niente.

journal.cli.journal.facet.merge.preview.entities = { $count ->
    [one] { $count } entità è collegata in entrambe le facette; finirebbe con un solo collegamento, tenendo ogni nota.
    [many] { $count } di entità sono collegate in entrambe le facette; ognuna finirebbe con un solo collegamento, tenendo ogni nota.
   *[other] { $count } entità sono collegate in entrambe le facette; ognuna finirebbe con un solo collegamento, tenendo ogni nota.
}

journal.cli.journal.facet.merge.preview.fields = Campi di entità in cui '{ $destination }' terrebbe il proprio valore, diverso:

journal.cli.journal.facet.merge.preview.files = File che entrambe le facette hanno e che non si possono combinare. '{ $destination }' terrebbe la propria copia e la copia in '{ $source }' andrebbe nel cestino:

journal.cli.journal.facet.merge.preview.heading = Esecuzione di prova, non è stato cambiato niente. L'unione di '{ $source }' in '{ $destination }' perderebbe:

journal.cli.journal.facet.merge.preview.notes = { $count ->
    [one] { $count } nota riceverebbe un nuovo numero, dopo le note già lì.
    [many] { $count } di note riceverebbero un nuovo numero, dopo le note già lì.
   *[other] { $count } note riceverebbero un nuovo numero, dopo le note già lì.
}

journal.cli.journal.facet.merge.preview.records = Registri che verrebbero scartati perché si tiene un registro diverso con lo stesso id (si tiene il primo registro di ogni id, leggendo prima '{ $destination }'):

journal.cli.journal.facet.merge.preview.settings = Le impostazioni nel { $declaration } di '{ $source }', che non verrebbero trasferite.

journal.cli.journal.facet.merge.queue.failed = la riparazione è finita, ma non è stato possibile mettere in coda il suo aggiornamento della ricerca: { $error }

journal.cli.journal.facet.merge.scratch.failed = non è stato possibile rimuovere la copia di lavoro dell'esecuzione di prova { $path }: { $error }; non è stato cambiato nient'altro, e quella cartella si può eliminare

journal.cli.journal.facet.merge.source.absent = '{ $source }' non è una facetta

journal.cli.journal.facet.merge.source.declared = '{ $source }' è una facetta dichiarata, non una cartella orfana

journal.cli.journal.facet.merge.source.only = '{ $source }' è l'unica facetta che non è silenziata, e un diario ne tiene almeno una; togli il silenzio a '{ $destination }' prima

journal.cli.journal.facet.merge.source.retired = '{ $source }' è un nome di facetta ritirato

journal.cli.journal.facet.merge.trash.failed = l'unione è finita, ma non è stato possibile spostare nel cestino i file che ha sostituito: { $error }; sono tenuti, e '{ $repair_command }' li sposta lì

journal.cli.journal.facet.name.encoding = il nome della facetta non è { $encoding }

journal.cli.journal.facet.name.unsafe = dichiarazione di facetta non sicura per { $slug }

journal.cli.journal.facet.notify.queue = non è stato possibile mettere in coda l'aggiornamento della ricerca: { $error }

journal.cli.journal.facet.notify.search = la ricerca non è stata aggiornata: { $reason }

journal.cli.journal.facet.path.encoding = { $path } non è { $encoding }

journal.cli.journal.facet.path.plain = { $path } non è un percorso semplice

journal.cli.journal.facet.record.declaration.damaged = la dichiarazione di facetta è danneggiata: { $detail }

journal.cli.journal.facet.record.declaration.exists = la dichiarazione di facetta esiste già: { $path }

journal.cli.journal.facet.record.declaration.invalid = la dichiarazione di facetta non è un oggetto: { $path }

journal.cli.journal.facet.record.declaration.missing = la dichiarazione di facetta manca: { $path }

journal.cli.journal.facet.record.declaration.unreadable = non è stato possibile leggere la dichiarazione di facetta: { $detail }

journal.cli.journal.facet.record.id.duplicate = identificatore di facetta ben formato duplicato già sul disco: { $id }

journal.cli.journal.facet.record.id.entropy = non è stato possibile ottenere byte casuali per l'identificatore: { $error }

journal.cli.journal.facet.record.id.exhausted = tentativi esauriti durante la generazione di un identificatore di facetta univoco

journal.cli.journal.facet.record.link.invalid = il collegamento di entità della facetta non è un oggetto: { $path }

journal.cli.journal.facet.record.malformed.activity = non è stato possibile leggere le definizioni di attività in { $path } riga { $line }: { $reason }

journal.cli.journal.facet.record.malformed.observation = osservazione malformata { $source } riga { $line }: { $reason }

journal.cli.journal.facet.record.marker.invalid = il marcatore di completamento della riparazione del collegamento di entità della facetta è vuoto o malformato: { $path }

journal.cli.journal.facet.record.minimum = la facetta '{ $facet }' è l'unica facetta attiva; un diario ne tiene almeno una

journal.cli.journal.facet.record.missing.names = oggetto { $names_field } mancante

journal.cli.journal.facet.record.name.retired = il nome di facetta '{ $name }' apparteneva a una facetta che è stata unita o rinominata, e ora porta a essa

journal.cli.journal.facet.record.not.object = non è un oggetto { $json_format }

journal.cli.journal.facet.record.reason.content = atteso un oggetto con contenuto di stringa non vuoto

journal.cli.journal.facet.record.reason.history = campo { $history_field } non valido

journal.cli.journal.facet.record.reason.id = { $id_field } deve essere un numero intero senza segno

journal.cli.journal.facet.record.reason.json = { $json_format } non valido

journal.cli.journal.facet.record.reason.object = atteso un oggetto

journal.cli.journal.facet.record.reason.relation = atteso un oggetto con contenuto di stringa non vuoto o con relazione

journal.cli.journal.facet.record.reason.retired = campo { $retired_field } non valido

journal.cli.journal.facet.record.retired.conflict = il nome di facetta '{ $name }' è già registrato come ritirato con un esito diverso

journal.cli.journal.facet.record.retired.damaged = non è stato possibile leggere { $retired_path } ({ $detail }); esegui '{ $repair_command }'

journal.cli.journal.facet.record.source.snapshot = istantanea acquisita

journal.cli.journal.facet.retire.argument = { $retire_flag } richiede { $name_argument } e poi { $into_facet } o { $deleted_flag }

journal.cli.journal.facet.retire.deleted = Registrato '{ $name }' come eliminato. la ricerca si sta aggiornando.

journal.cli.journal.facet.retire.encoding = i nomi devono essere { $encoding }

journal.cli.journal.facet.retire.folder = '{ $name }' è una cartella in questo diario; si può ritirare solo un nome che non esiste più

journal.cli.journal.facet.retire.merged = Registrato '{ $name }' come unito in '{ $target }'. la ricerca si sta aggiornando.

journal.cli.journal.facet.retire.name = '{ $name }' non è un nome di facetta

journal.cli.journal.facet.retire.target = '{ $target }' non è una facetta con un id stabile; esegui '{ $backfill_command }' prima

journal.cli.journal.facet.transaction.exists = il percorso della transazione esiste già: { $path }

journal.cli.journal.facet.transaction.rollback = la transazione non è riuscita ed è stata ripristinata: { $primary }

journal.cli.journal.facet.transaction.rollback.failed = la transazione non è riuscita ({ $primary }); anche il ripristino non è riuscito ({ $rollback_error })

journal.cli.journal.facet.trash.absent = non è più nel cestino

journal.cli.journal.facet.trash.busy = è già in corso il recupero o la cancellazione

journal.cli.journal.facet.trash.io = non è stato possibile modificare il tuo cestino: { $error }

journal.cli.journal.facet.trash.missing = non esiste { $item } nel tuo diario

journal.cli.journal.facet.trash.unfinished = è nel cestino, che finirà di metterlo via

journal.cli.journal.facet.trash.unknown = questa build non può recuperarlo

journal.cli.journal.help.overview = { $invocation } - il diario su questo computer{ $line_break }{ $line_break }Uso: { $syntax }{ $line_break }{ $line_break }{ $old_command } è passato a '{ $invocation }'; usa quel comando per le operazioni del diario in locale.{ $line_break }{ $line_break }Comandi in locale:{ $line_break }{ $commands }{ $line_break }{ $line_break }Comandi di processo:{ $line_break }{ $processes }{ $line_break }{ $line_break }Comandi del diario in locale:{ $line_break }{ $local }{ $line_break }{ $line_break }Opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_4 }Mostra questo aiuto{ $line_break }{ $syntax_continuation_17 }{ $line_break }{ $syntax_continuation_18 }

journal.cli.journal.help.path = uso: { $syntax }{ $line_break }{ $line_break }Stampa il percorso risolto del diario.

journal.cli.journal.help.root = uso: { $syntax }{ $line_break }{ $line_break }Stampa la radice del progetto.

journal.cli.journal.help.setup.copy = uso: { $syntax }

journal.cli.journal.help.status = uso: { $syntax }{ $line_break }{ $line_break }Stampa se il diario esiste e quanti giorni ha.

journal.cli.journal.help.usage = Uso: { $syntax }

journal.cli.journal.helper.inspect = { $code }: non è stato possibile ispezionare l'eseguibile nativo del diario: { $error }

journal.cli.journal.helper.missing.mac = { $code }: { $path }. Reinstalla l'app del diario.

journal.cli.journal.helper.missing.other = { $code }: { $path }. Reinstalla { $package }.

journal.cli.journal.helper.mode.mac = { $code }: { $path }. Reinstalla l'app del diario.

journal.cli.journal.helper.mode.other = { $code }: { $path }. Reinstalla { $package }.

journal.cli.journal.host.path.utf8 = risoluzione del percorso del diario nativo non riuscita: il percorso risolto del diario non è un UTF-8 valido

journal.cli.journal.host.status.absent = Diario: { $path }{ $line_break }Fonte: { $source }{ $line_break }Esiste: no{ $line_break }Giorni: { $days }

journal.cli.journal.host.status.directory = stato del diario nativo non riuscito: la radice del diario non è una directory: { $path }

journal.cli.journal.host.status.exists = Diario: { $path }{ $line_break }Fonte: { $source }{ $line_break }Esiste: sì{ $line_break }Giorni: { $days }

journal.cli.journal.host.status.inspect = stato del diario nativo non riuscito: non è stato possibile ispezionare { $path }: { $error }

journal.cli.journal.host.status.utf8 = stato del diario nativo non riuscito: il percorso risolto del diario non è un UTF-8 valido

journal.cli.journal.indexer.argument.argument = argomento imprevisto

journal.cli.journal.indexer.argument.budget = { $budget_seconds_flag } ha bisogno di un numero positivo

journal.cli.journal.indexer.argument.drain = { $drain_flag } richiede { $apply_flag }

journal.cli.journal.indexer.argument.duplicate = argomento imprevisto o duplicato

journal.cli.journal.indexer.argument.file = { $rescan_file_flag } richiede { $path }

journal.cli.journal.indexer.argument.ios = non disponibile su { $ios }

journal.cli.journal.indexer.argument.missing.agent = { $agent_flag } richiede { $agent }

journal.cli.journal.indexer.argument.missing.day = { $day_flag } richiede { $day }

journal.cli.journal.indexer.argument.missing.day.from = { $day_from_flag } richiede { $day }

journal.cli.journal.indexer.argument.missing.day.to = { $day_to_flag } richiede { $day }

journal.cli.journal.indexer.argument.missing.facet = { $facet_flag } richiede { $facet }

journal.cli.journal.indexer.argument.missing.limit = { $limit_flag } richiede un numero intero non negativo

journal.cli.journal.indexer.argument.missing.offset = { $offset_flag } richiede un numero intero non negativo

journal.cli.journal.indexer.argument.missing.stream = { $stream_flag } richiede { $stream }

journal.cli.journal.indexer.argument.missing.top = { $top_flag } richiede un numero intero non negativo

journal.cli.journal.indexer.argument.rescan.conflict = { $rescan_file_flag } non si può combinare con { $rescan_flag } o { $rescan_full_flag }

journal.cli.journal.indexer.argument.unsupported = file non supportato

journal.cli.journal.indexer.build.building = costruzione: in costruzione

journal.cli.journal.indexer.build.complete = costruzione: completa

journal.cli.journal.indexer.build.unknown = costruzione: sconosciuta

journal.cli.journal.indexer.calendar.at = riga { $line }: { $reason }

journal.cli.journal.indexer.calendar.import = note del calendario in { $rel } saltate: { $error }

journal.cli.journal.indexer.calendar.line.repeat = la riga { $line } della nota del calendario ripete un evento

journal.cli.journal.indexer.calendar.line.skip = la riga { $line } della nota del calendario saltata: { $reason }

journal.cli.journal.indexer.calendar.segment = note del calendario saltate: { $rel } non è in un segmento

journal.cli.journal.indexer.calendar.settings = l'evento del calendario in { $path } è elencato una volta che si può leggere il file delle impostazioni: { $error }

journal.cli.journal.indexer.calendar.source = le note del calendario si leggono solo dai flussi del calendario e dalle importazioni del calendario: { $rel }

journal.cli.journal.indexer.calendar.unlisted = nota del calendario in { $path } non elencata: { $reason }

journal.cli.journal.indexer.calendar.validation.account.must.be.a.string.or.null = { $account_field } deve essere una stringa o { $null_key }

journal.cli.journal.indexer.calendar.validation.all.day.must.be.true.or.false = { $all_day_field } deve essere { $true_key } o { $false_key }

journal.cli.journal.indexer.calendar.validation.attendee.status.must.be.a.string.or.null = partecipante { $status_field } deve essere una stringa o { $null_key }

journal.cli.journal.indexer.calendar.validation.attendee.you.must.be.true.or.false = partecipante { $you_field } deve essere { $true_key } o { $false_key }

journal.cli.journal.indexer.calendar.validation.attendees.must.be.an.array = { $attendees_field } deve essere un array

journal.cli.journal.indexer.calendar.validation.change.must.be.added.changed.or.removed = { $change_field } deve essere { $added_changed_or_removed_key }

journal.cli.journal.indexer.calendar.validation.changed.at.falls.outside.the.segment.window = { $changed_at_field } è fuori dalla finestra del segmento

journal.cli.journal.indexer.calendar.validation.changed.at.must.be.a.time.with.an.offset = { $changed_at_field } deve essere un orario con un offset

journal.cli.journal.indexer.calendar.validation.each.attendee.must.be.an.object = ogni partecipante deve essere un oggetto

journal.cli.journal.indexer.calendar.validation.end.cannot.be.read.as.a.date.or.time = { $end_field } non si può leggere come data o orario

journal.cli.journal.indexer.calendar.validation.end.is.before.start = { $end_field } è prima dell'inizio

journal.cli.journal.indexer.calendar.validation.end.must.be.a.string.or.null = { $end_field } deve essere una stringa o { $null_key }

journal.cli.journal.indexer.calendar.validation.event.and.occurrence.repeat.an.earlier.line = { $event_field } e l'occorrenza ripetono una riga precedente

journal.cli.journal.indexer.calendar.validation.event.must.be.a.non.empty.string = { $event_field } deve essere una stringa non vuota

journal.cli.journal.indexer.calendar.validation.expanded.must.be.true.when.present = { $expanded_field } deve essere { $true_key } quando è presente

journal.cli.journal.indexer.calendar.validation.field.missing = { $key } manca

journal.cli.journal.indexer.calendar.validation.field.type = { $key } deve essere una stringa o { $null_input }

journal.cli.journal.indexer.calendar.validation.file.encoding = il file non è { $encoding }

journal.cli.journal.indexer.calendar.validation.file.holds.no.notes = il file non contiene note

journal.cli.journal.indexer.calendar.validation.file.lines = il file contiene più di 5.000 note

journal.cli.journal.indexer.calendar.validation.file.size = il file contiene più di 8 { $unit }

journal.cli.journal.indexer.calendar.validation.filename = il nome del file deve essere { $prefix } seguito da 64 cifre esadecimali minuscole e { $suffix }

journal.cli.journal.indexer.calendar.validation.import.must.be.a.non.empty.string = { $import_field } deve essere una stringa non vuota

journal.cli.journal.indexer.calendar.validation.imported.at.must.be.a.time.with.an.offset = { $imported_at_key } deve essere un orario con un offset

journal.cli.journal.indexer.calendar.validation.line.is.empty = la riga è vuota

journal.cli.journal.indexer.calendar.validation.line.is.not.a.json.object = la riga non è un oggetto { $json_key }

journal.cli.journal.indexer.calendar.validation.occurrence.cannot.be.read.as.a.date.or.time = { $occurrence_field } non si può leggere come data o orario

journal.cli.journal.indexer.calendar.validation.occurrence.must.be.a.string.or.null = { $occurrence_field } deve essere una stringa o { $null_key }

journal.cli.journal.indexer.calendar.validation.organizer.must.be.an.object.or.null = { $organizer_field } deve essere un oggetto o { $null_key }

journal.cli.journal.indexer.calendar.validation.person.field = { $what } { $key } deve essere una stringa o { $null_input }

journal.cli.journal.indexer.calendar.validation.segment.grid = il segmento deve indicare una finestra di 300 secondi sulla griglia del diario

journal.cli.journal.indexer.calendar.validation.segment.must.name.a.span.on.its.day = il segmento deve indicare un intervallo nel suo giorno

journal.cli.journal.indexer.calendar.validation.start.and.end.must.be.dates.exactly.when.all.day.is.true = { $start_field } e { $end_field } devono essere delle date esattamente quando { $all_day_field } è { $true_key }

journal.cli.journal.indexer.calendar.validation.start.and.end.must.both.carry.an.offset.or.both.have.none = { $start_field } e { $end_field } devono portare entrambi un offset o non averne nessuno

journal.cli.journal.indexer.calendar.validation.start.cannot.be.read.as.a.date.or.time = { $start_field } non si può leggere come data o orario

journal.cli.journal.indexer.calendar.validation.start.is.missing = { $start_field } manca

journal.cli.journal.indexer.calendar.validation.version = { $v_field } deve essere 1

journal.cli.journal.indexer.chunker.activity = versione non supportata dell'attività desktop

journal.cli.journal.indexer.chunker.calendar = nota del calendario saltata: { $reason }

journal.cli.journal.indexer.chunker.lines = { $collapsed ->
    [one] Spazio bianco compresso su { $collapsed } riga Markdown troppo lunga durante l'indicizzazione
    [many] Spazio bianco compresso su { $collapsed } di righe Markdown troppo lunghe durante l'indicizzazione
   *[other] Spazio bianco compresso su { $collapsed } righe Markdown troppo lunghe durante l'indicizzazione
    }

journal.cli.journal.indexer.classification.absent = classificazioni: database assente

journal.cli.journal.indexer.classification.complete = classificazioni: complete (elaborati { $processed }, non classificati { $unclassified })

journal.cli.journal.indexer.classification.incomplete = classificazioni: sidecar incompleto al cursore '{ $cursor }' (esegui '{ $repair }')

journal.cli.journal.indexer.classification.mismatch = classificazioni: mancata corrispondenza della copertura al cursore '{ $cursor }' (elaborati { $processed }, rimanenti { $remaining }, mancanti { $missing }, non classificati { $unclassified })

journal.cli.journal.indexer.classification.missing = { $missing ->
    [one] mancata corrispondenza della copertura della classificazione dei chunk: { $missing } riga mancante
    [many] mancata corrispondenza della copertura della classificazione dei chunk: { $missing } di righe mancanti
   *[other] mancata corrispondenza della copertura della classificazione dei chunk: { $missing } righe mancanti
    }

journal.cli.journal.indexer.classification.progress = classificazioni: in corso al cursore '{ $cursor }' (elaborati { $processed }, rimanenti { $remaining }, mancanti { $missing }, non classificati { $unclassified })

journal.cli.journal.indexer.classification.stall = backfill della classificazione dei chunk in stallo: { $err }

journal.cli.journal.indexer.classification.stalled = classificazioni: in stallo a '{ $path }' (elaborati { $processed }, rimanenti { $remaining }, mancanti { $missing }, non classificati { $unclassified })

journal.cli.journal.indexer.classification.unready = classificazioni: non pronte al cursore '{ $cursor }' (esegui '{ $repair_command }')

journal.cli.journal.indexer.database.absent = indice: assente

journal.cli.journal.indexer.database.files = file: { $discovered } su disco, { $indexed } indicizzati, { $current } correnti, { $stale } non aggiornati, { $missing } mancanti, { $failed } falliti, { $orphaned } orfani, { $retained } tenuti fino a una nuova scansione completa, { $ineligible_rows } non più indicizzabili ma ancora nella ricerca, { $unreadable } illeggibili, { $ineligible } non indicizzabili

journal.cli.journal.indexer.database.present = indice: presente

journal.cli.journal.indexer.database.sqlite = non è un database { $sqlite }

journal.cli.journal.indexer.database.unreadable = indice: illeggibile ({ $reason })

journal.cli.journal.indexer.discovery.listing = attraversamento del glob non riuscito: il tentativo di leggere `{ $path }` ha prodotto un errore: { $error }

journal.cli.journal.indexer.discovery.pattern = pattern glob non valido: { $error }

journal.cli.journal.indexer.discovery.prefix = il percorso scoperto { $path } non è sotto la radice { $root }

journal.cli.journal.indexer.discovery.relative = il percorso scoperto non è un { $encoding } valido: { $path }

journal.cli.journal.indexer.discovery.root = la radice del glob non è un { $encoding } valido: { $path }

journal.cli.journal.indexer.drain.result = { $processed ->
    [one] applicato { $processed } aggiornamento della ricerca; { $failed } falliti; { $remaining } { $remaining ->
        [one] resta
        [many] restano
       *[other] restano
    } in coda.
    [many] applicati { $processed } di aggiornamenti della ricerca; { $failed } falliti; { $remaining } { $remaining ->
        [one] resta
        [many] restano
       *[other] restano
    } in coda.
   *[other] applicati { $processed } aggiornamenti della ricerca; { $failed } falliti; { $remaining } { $remaining ->
        [one] resta
        [many] restano
       *[other] restano
    } in coda.
    }

journal.cli.journal.indexer.edge.blob = la riga dell'arco contiene un valore blob non supportato

journal.cli.journal.indexer.edge.day = Giorno di arco non valido: { $day }

journal.cli.journal.indexer.edge.dst = la riga dell'arco richiede una stringa non vuota { $dst_field }

journal.cli.journal.indexer.edge.event = il timestamp dell'evento è fuori dall'intervallo { $integer }

journal.cli.journal.indexer.edge.integer = il campo intero dell'arco { $field } è fuori dall'intervallo { $integer }

journal.cli.journal.indexer.edge.json = analisi del { $json } della fonte dell'arco non riuscita per { $path }: { $error }

journal.cli.journal.indexer.edge.kind = Tipo di arco sconosciuto: { $kind }

journal.cli.journal.indexer.edge.path = percorso delle osservazioni non valido: { $path }

journal.cli.journal.indexer.edge.payload = il payload { $source } deve essere un oggetto { $json }, trovato { $value_type }

journal.cli.journal.indexer.edge.refresh = Si salta il refresh dell'arco per { $rel }: { $error }

journal.cli.journal.indexer.edge.relation = manca { $kind_field } nella relazione di osservazione

journal.cli.journal.indexer.edge.segment.day = Giorno di segmento non valido: { $day }

journal.cli.journal.indexer.edge.segment.key = Chiave di segmento non valida: { $segment }

journal.cli.journal.indexer.edge.segment.time = il timestamp del segmento è fuori dall'intervallo { $integer }

journal.cli.journal.indexer.edge.src = la riga dell'arco richiede una stringa non vuota { $src_field }

journal.cli.journal.indexer.edge.string = non si può riprodurre { $python } per il valore { $field } di tipo { $value_type }

journal.cli.journal.indexer.edge.target = { $target_field } della relazione di osservazione deve essere una stringa non vuota

journal.cli.journal.indexer.edge.type = il tipo di arco per { $source } deve essere una stringa, trovato { $value_type }

journal.cli.journal.indexer.edge.unsupported = il campo dell'arco { $field } non supporta { $value_type }

journal.cli.journal.indexer.error.file = rescan-file non riuscito: { $detail }

journal.cli.journal.indexer.error.rebuild = ricostruzione dell'arco non riuscita: { $detail }

journal.cli.journal.indexer.error.reset = reimpostazione non riuscita: { $detail }

journal.cli.journal.indexer.error.scan = scansione non riuscita: { $detail }

journal.cli.journal.indexer.files.activity.membership.insert = inserimento dell'appartenenza all'attività non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.activity.provenance.insert = inserimento della provenienza dell'attività non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.listing = elenco del calendario non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.calendar.note.insert = inserimento della nota del calendario non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.group.offset.insert = inserimento dell'offset del gruppo di chunk non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.insert = inserimento del chunk non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.chunk.source.mapping = mappatura della fonte del chunk non riuscita per { $rel }: { $error }

journal.cli.journal.indexer.files.classification.insert = inserimento della classificazione non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.content.read = lettura del contenuto non riuscita per { $rel }: { $error }

journal.cli.journal.indexer.files.facet = scansione della dichiarazione della facetta non riuscita per { $rel }: { $error }

journal.cli.journal.indexer.files.hidden = { $rel } resta fuori dalla ricerca per ora: { $cause }

journal.cli.journal.indexer.files.memory.chain.serialization = serializzazione della catena del ricordo non riuscita per { $rel }: { $error }

journal.cli.journal.indexer.files.memory.missing = manca l'originale del ricordo convalidato per { $rel }

journal.cli.journal.indexer.files.memory.origin.serialization = serializzazione dell'origine del ricordo non riuscita per { $rel }: { $error }

journal.cli.journal.indexer.files.memory.original.insert = inserimento dell'originale del ricordo non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.files.memory.utf = l'originale del ricordo non è { $utf } per { $rel }: { $error }

journal.cli.journal.indexer.files.partial = { $rel } resta in parte fuori dalla ricerca per ora: { $cause }

journal.cli.journal.indexer.files.screen.text.source.insert = inserimento della fonte del testo dello schermo non riuscito per { $rel }: { $error }

journal.cli.journal.indexer.generation.locked = generazione: { $generation } (questa versione legge fino a { $known }; scrivibile: no)

journal.cli.journal.indexer.generation.writable = generazione: { $generation } (questa versione legge fino a { $known }; scrivibile: sì)

journal.cli.journal.indexer.help.drain = Uso: { $syntax }{ $line_break }applica gli aggiornamenti della ricerca in coda. { $force_flag } include i nuovi tentativi ritardati.{ $line_break }{ $budget_seconds_flag } ferma lo svuotamento dopo N secondi. un aggiornamento dell'intero diario non parte con meno di 300 secondi rimasti e resta in coda.

journal.cli.journal.indexer.help.main = Uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $syntax_continuation_4 }

journal.cli.journal.indexer.help.path = Uso: { $syntax }

journal.cli.journal.indexer.help.status = Uso: { $syntax }

journal.cli.journal.indexer.hidden.activity = non si capisce se questa attività è nascosta: { $rel } non si può leggere: { $error }

journal.cli.journal.indexer.hidden.damaged = non si capisce se questa attività è nascosta: { $path } ha una riga che non è un registro

journal.cli.journal.indexer.hidden.days = non si capisce quali giorni dei dati del corpo sono nascosti: { $path } non si può leggere

journal.cli.journal.indexer.hidden.trash = non si capisce che cosa il diario sta ricreando: { $error }

journal.cli.journal.indexer.hidden.week = non si capisce che cosa resta fuori da questa pagina della settimana: { $path } non si può leggere

journal.cli.journal.indexer.hidden.weeks = non si capisce che cosa resta fuori dalle pagine della settimana: { $path } non si può leggere

journal.cli.journal.indexer.merge.commit = l'indice è corretto e semplicemente non compattato: non è stato possibile eseguire il commit dell'unione { $fts }: { $error }

journal.cli.journal.indexer.merge.failed = l'indice è corretto e semplicemente non compattato: unione { $fts } non riuscita: { $error }

journal.cli.journal.indexer.merge.start = l'indice è corretto e semplicemente non compattato: non è stato possibile avviare la transazione di unione { $fts }: { $error }

journal.cli.journal.indexer.migration.child = la migrazione dell'aggregato del segmento ha conservato { $rel_segment }: il file figlio non è corrente: { $rel }

journal.cli.journal.indexer.migration.discovery = la migrazione dell'aggregato del segmento ha conservato { $rel_segment }: scoperta del talento non riuscita: { $error }

journal.cli.journal.indexer.migration.files = i file dell'indice restano dopo la migrazione: { $paths }

journal.cli.journal.indexer.migration.mtime = la migrazione dell'aggregato del segmento ha conservato { $rel_segment }: lettura di mtime non riuscita per { $rel }: { $error }

journal.cli.journal.indexer.migration.retained = la migrazione dell'aggregato del segmento ha conservato { $rel }: { $error }

journal.cli.journal.indexer.migration.unclassified = la migrazione dell'aggregato del segmento ha conservato { $rel_segment }: Markdown di talento non classificato { $rel }

journal.cli.journal.indexer.original.incomplete = l'originale del ricordo è incompleto per { $rel }: { $state }

journal.cli.journal.indexer.original.retained = non è stato possibile controllare l'originale del ricordo per { $rel }; indice in cache conservato: { $state }

journal.cli.journal.indexer.original.state.absent = assente

journal.cli.journal.indexer.original.state.corrupt = corrotto

journal.cli.journal.indexer.original.state.deleted = eliminato

journal.cli.journal.indexer.original.state.ready = pronto

journal.cli.journal.indexer.original.state.staged = preparato

journal.cli.journal.indexer.original.state.unavailable = non disponibile ({ $detail })

journal.cli.journal.indexer.original.state.unready = non pronto

journal.cli.journal.indexer.path.ready = consultazione del percorso: pronta

journal.cli.journal.indexer.path.unready = consultazione del percorso: non pronta

journal.cli.journal.indexer.progress.absent = classificazione: assente

journal.cli.journal.indexer.progress.complete = classificazione: completa

journal.cli.journal.indexer.progress.incomplete = classificazione: incompleta

journal.cli.journal.indexer.progress.stalled = classificazione: in stallo a { $path }

journal.cli.journal.indexer.progress.unknown = classificazione: in stallo a un percorso sconosciuto

journal.cli.journal.indexer.query.column.agent = Agente

journal.cli.journal.indexer.query.column.day = Giorno

journal.cli.journal.indexer.query.column.facet = Facetta

journal.cli.journal.indexer.query.empty = l'indice del diario è vuoto: { $path }

journal.cli.journal.indexer.query.error = { $command }: { $detail }

journal.cli.journal.indexer.query.input = lettura di stdin non riuscita: { $error }

journal.cli.journal.indexer.query.locked = l'indice del diario è bloccato ({ $path }): { $detail }

journal.cli.journal.indexer.query.more = ... +{ $remaining } in più

journal.cli.journal.indexer.query.none = Nessun risultato trovato

journal.cli.journal.indexer.query.output = scrittura su stdout non riuscita: { $error }

journal.cli.journal.indexer.query.owner = l'interrogazione di connessione richiede il titolare

journal.cli.journal.indexer.query.prompt = cerca>

journal.cli.journal.indexer.query.showing = { $total ->
    [one] Visualizzazione di { $first }-{ $last } di { $total } risultato
    [many] Visualizzazione di { $first }-{ $last } di { $total } di risultati
   *[other] Visualizzazione di { $first }-{ $last } di { $total } risultati
    }

journal.cli.journal.indexer.query.unreadable = l'indice del diario non è leggibile ({ $path }): { $detail }

journal.cli.journal.indexer.receipt.absent = ultima scansione: non registrata

journal.cli.journal.indexer.receipt.full.finished = ultima scansione: completa a { $time } ms, indicizzati { $indexed }, rimossi { $removed }, falliti { $failed }

journal.cli.journal.indexer.receipt.full.stopped = ultima scansione: completa a { $time } ms interrotta: { $error }

journal.cli.journal.indexer.receipt.light.finished = ultima scansione: leggera a { $time } ms, indicizzati { $indexed }, rimossi { $removed }, falliti { $failed }

journal.cli.journal.indexer.receipt.light.stopped = ultima scansione: leggera a { $time } ms interrotta: { $error }

journal.cli.journal.indexer.rel.absolute = { $rel } deve essere journal-{ $rel }ative

journal.cli.journal.indexer.rel.backslash = { $rel } deve usare separatori { $posix }

journal.cli.journal.indexer.rel.empty = { $rel } deve essere non vuoto

journal.cli.journal.indexer.rel.invalidcomponent = { $rel } non deve contenere componenti vuoti, { $current_component } o { $parent_component }

journal.cli.journal.indexer.rel.trash = { $rel } non deve nominare il cestino

journal.cli.journal.indexer.scan.calendar.invites = inviti del calendario non ancora collegati: { $error }

journal.cli.journal.indexer.scan.connections = Nessuna nuova connessione è stata aggiunta in questa nuova scansione. Esegui { $rebuild_command } per aggiornare ora quelle ancora in attesa.

journal.cli.journal.indexer.scan.connections.failed = connessioni non controllate rispetto alle modifiche delle entità: { $error }

journal.cli.journal.indexer.scan.deferred = una passata completa di ricerca sta già elaborando queste modifiche, quindi questo controllo le ha lasciate a quella. esegui { $status_command } per i dettagli.

journal.cli.journal.indexer.scan.edge.skip = Estrazione degli archi saltata per { $rel }: { $error }

journal.cli.journal.indexer.scan.failed = { $indexed ->
    [one] Indicizzato { $indexed } file, rimossi { $removed }, saltati { $skipped }, falliti { $failed }
    [many] Indicizzati { $indexed } di file, rimossi { $removed }, saltati { $skipped }, falliti { $failed }
   *[other] Indicizzati { $indexed } file, rimossi { $removed }, saltati { $skipped }, falliti { $failed }
    }

journal.cli.journal.indexer.scan.failure = { $failed ->
    [one] { $command }: non è stato possibile indicizzare { $failed } aggiornamento; vedi gli avvisi sopra
    [many] { $command }: non è stato possibile indicizzare { $failed } di aggiornamenti; vedi gli avvisi sopra
   *[other] { $command }: non è stato possibile indicizzare { $failed } aggiornamenti; vedi gli avvisi sopra
    }

journal.cli.journal.indexer.scan.invalid.segment = Estrazione degli archi saltata per { $rel }: chiave di segmento non valida { $segment }

journal.cli.journal.indexer.scan.mtime = lettura di mtime non riuscita per { $rel }: { $error }

journal.cli.journal.indexer.scan.retained = { $count ->
    [one] la scansione leggera ha conservato { $count } riga non aggiornata per il giorno { $day }: la scoperta non ha prodotto file per quel giorno; esegui di nuovo con { $full_flag } per rimuoverla
    [many] la scansione leggera ha conservato { $count } di righe non aggiornate per il giorno { $day }: la scoperta non ha prodotto file per quel giorno; esegui di nuovo con { $full_flag } per rimuoverle
   *[other] la scansione leggera ha conservato { $count } righe non aggiornate per il giorno { $day }: la scoperta non ha prodotto file per quel giorno; esegui di nuovo con { $full_flag } per rimuoverle
    }

journal.cli.journal.indexer.scan.summary = { $indexed ->
    [one] Indicizzato { $indexed } file, rimossi { $removed }, saltati { $skipped }
    [many] Indicizzati { $indexed } di file, rimossi { $removed }, saltati { $skipped }
   *[other] Indicizzati { $indexed } file, rimossi { $removed }, saltati { $skipped }
    }

journal.cli.journal.indexer.speaker.duplicate = trascrizione duplicata dell'arco del parlante { $field } { $sentence } per { $composite_id }

journal.cli.journal.indexer.speaker.missing = trascrizione dell'arco del parlante mancante per { $composite_id }

journal.cli.journal.indexer.speaker.unresolved = trascrizione dell'arco del parlante non risolta per { $composite_id }

journal.cli.journal.indexer.status.check = ultimo controllo della ricerca: a { $time } ms, indicizzati { $indexed }, rimossi { $removed }

journal.cli.journal.indexer.status.check.absent = ultimo controllo della ricerca: non registrato

journal.cli.journal.indexer.status.check.overdue = ultimo controllo della ricerca: in ritardo, da allora è in esecuzione senza finire

journal.cli.journal.indexer.status.failure = fallimento: { $reason }

journal.cli.journal.indexer.status.health = stato: { $state } ({ $description })

journal.cli.journal.indexer.status.queued = in coda: { $queued }

journal.cli.journal.indexer.status.removals = rimozioni ancora in attesa: { $removals }

journal.cli.journal.indexer.status.wait.days = il più vecchio in coda: in attesa da { $days } d { $hours } h

journal.cli.journal.indexer.status.wait.hours = il più vecchio in coda: in attesa da { $hours } h { $minutes } min

journal.cli.journal.indexer.status.wait.minutes = il più vecchio in coda: in attesa da { $minutes } min

journal.cli.journal.indexer.status.wait.seconds = il più vecchio in coda: in attesa da { $seconds } s

journal.cli.journal.indexer.store.admission = ammissione dello scrittore dell'indice non riuscita per { $operation }: { $cause }

journal.cli.journal.indexer.store.busy = scrittore dell'indice occupato per { $operation }: { $cause }

journal.cli.journal.indexer.store.encoding = il percorso non è { $encoding } valido: { $path }

journal.cli.journal.indexer.store.generation = l'indice di ricerca è stato scritto da una versione più recente di { $brand } (generazione dell'indice { $found }; questa versione legge fino a { $known }), quindi questa versione lo lascia invariato

journal.cli.journal.indexer.store.lookup.cause = { $lookup_command }: { $cause }

journal.cli.journal.indexer.store.missing = file non trovato: { $path }

journal.cli.journal.indexer.store.original = non è stato possibile controllare l'originale del ricordo in { $path }; indice in cache conservato; riprova quando la fonte è leggibile: { $detail }

journal.cli.journal.indexer.store.outside = il file è fuori dalla directory del diario: { $path }

journal.cli.journal.indexer.stream.json = marcatore di flusso { $syntax } non riuscito per { $path }: { $error }

journal.cli.journal.indexer.stream.path = percorso del marcatore di flusso non riuscito per { $rel_segment }: { $error }

journal.cli.journal.indexer.stream.read = lettura del marcatore di flusso non riuscita per { $path }: { $error }

journal.cli.journal.indexer.warning = avviso: { $warning }

journal.cli.journal.local.directory = non è una directory reale: { $path }

journal.cli.journal.local.failure = { $invocation } { $token }: { $message }

journal.cli.journal.local.resolution.config = risoluzione del diario non riuscita: decodifica della configurazione non riuscita

journal.cli.journal.local.resolution.home = risoluzione del diario non riuscita: home non disponibile

journal.cli.journal.local.unknown = operazione locale sconosciuta

journal.cli.journal.notify.error.facet = { $invocation }: errore: { $option } non ha più supporto. Metti la selezione della facetta in { $action_flag }; per esempio, { $action_flag } { $example }.

journal.cli.journal.notify.error.integer = { $invocation }: errore: argomento { $auto_dismiss_flag }: valore intero non valido: '{ $value }'

journal.cli.journal.notify.error.message = { $invocation }: errore: i seguenti argomenti sono obbligatori: { $positional }

journal.cli.journal.notify.error.unknown = { $invocation }: errore: argomenti non riconosciuti: { $token }

journal.cli.journal.notify.error.value = { $invocation }: errore: argomento { $option }: atteso un argomento

journal.cli.journal.notify.failed = Non è stato possibile inviare la notifica ({ $channel } è in esecuzione?)

journal.cli.journal.notify.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                  { $message_options }{ $line_break }{ $line_break }Invia una notifica tramite { $channel }{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $positional }testo del messaggio di notifica{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $title_option }{ $gap_9 }titolo della notifica{ $line_break }{ $indent_2 }{ $icon_option }{ $gap_11 }{ $icon_set } nome dell'icona (predefinito: { $mailbox }){ $line_break }{ $indent_2 }{ $event_option }{ $gap_9 }nome dell'evento (predefinito: { $default_show }{ $line_break }{ $indent_2 }{ $action_option }{ $gap_7 }percorso URL da aprire al clic{ $line_break }{ $indent_2 }{ $app_option }{ $gap_13 }nome dell'app di origine{ $line_break }{ $indent_2 }{ $badge_option }{ $gap_9 }testo o numero del badge{ $line_break }{ $syntax_continuation_18 }{ $line_break }                        ignora automaticamente dopo N millisecondi{ $line_break }{ $indent_2 }{ $no_dismiss_option }{ $gap_10 }rendi la notifica non ignorabile{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Abilita l'output dettagliato{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Abilita il registro di debug

journal.cli.journal.notify.sent = Notifica inviata

journal.cli.journal.notify.utf8 = notify nativo del diario non riuscito: i tuoi argomenti non sono UTF-8 validi

journal.cli.journal.process.admission = ammissione del processo del diario non riuscita: { $detail }

journal.cli.journal.process.body = avvio del processo nativo del diario non riuscito: nessun corpo nativo per { $token }

journal.cli.journal.process.launch = avvio del processo nativo del diario non riuscito: { $error }

journal.cli.journal.resolution.config = risoluzione nativa del diario non riuscita: decodifica della configurazione non riuscita

journal.cli.journal.resolution.home = risoluzione nativa del diario non riuscita: home non disponibile

journal.cli.journal.root.inspect = risoluzione nativa della radice del progetto del diario non riuscita: non è stato possibile ispezionare l'eseguibile corrente: { $error }

journal.cli.journal.root.package = risoluzione nativa della radice del progetto del diario non riuscita: non è stato possibile trovare da { $path } il checkout dei sorgenti o il pacchetto { $product } installato

journal.cli.sol.call.activities.day.invalid = Errore: giorno non valido '{ $value }'

journal.cli.sol.call.activities.day.missing = Errore: il giorno è obbligatorio (passalo come argomento o imposta { $literal_1 }).

journal.cli.sol.call.activities.empty = Nessuna attività trovata.

journal.cli.sol.call.activities.exists = Errore: l'attività esiste già: { $detail }

journal.cli.sol.call.activities.facet.missing = Errore: la facetta è obbligatoria (passala come argomento o imposta { $literal_1 }).

journal.cli.sol.call.activities.facet.unknown = Errore: attività sconosciuta per la facetta '{ $facet }': { $activity_type }

journal.cli.sol.call.activities.kind.option = Errore: { $literal_1 } è obbligatorio.

journal.cli.sol.call.activities.kind.required = Errore: l'attività è obbligatoria.

journal.cli.sol.call.activities.participation.array = Errore: { $literal_1 } deve essere un array

journal.cli.sol.call.activities.participation.confidence = Errore: { $literal_1 }[{ $index }] '{ $literal_2 }' deve essere un numero

journal.cli.sol.call.activities.participation.context = Errore: { $literal_1 }[{ $index }] '{ $literal_2 }' deve essere una stringa

journal.cli.sol.call.activities.participation.name = Errore: { $literal_1 }[{ $index }] richiede una stringa non vuota '{ $literal_2 }'

journal.cli.sol.call.activities.participation.object = Errore: { $literal_1 }[{ $index }] deve essere un oggetto

journal.cli.sol.call.activities.participation.role = Errore: { $literal_1 }[{ $index }] ha un ruolo non valido '{ $role }' (deve essere uno tra ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }'])

journal.cli.sol.call.activities.participation.source = Errore: { $literal_1 }[{ $index }] ha una fonte non valida '{ $source }' (deve essere una tra ['{ $literal_2 }', '{ $literal_3 }', '{ $literal_4 }', '{ $literal_5 }', '{ $literal_6 }'])

journal.cli.sol.call.activities.patch.disallowed = Errore: campi di aggiornamento non ammessi: { $fields }

journal.cli.sol.call.activities.patch.empty = Errore: il payload di aggiornamento deve includere almeno un campo modificabile.

journal.cli.sol.call.activities.range.options = Errore: { $literal_1 } è incompatibile con { $literal_2 }/{ $literal_3 }.

journal.cli.sol.call.activities.range.reversed = Errore: { $literal_1 } ({ $end_day }) non deve essere prima di { $literal_2 } ({ $start_day })

journal.cli.sol.call.activities.segment.invalid = Errore: valore non valido per { $literal_1 } '{ $segment }' (atteso { $literal_2 })

journal.cli.sol.call.activities.source.create = Errore: { $literal_1 } deve essere '{ $literal_2 }'.

journal.cli.sol.call.activities.source.list = Errore: { $literal_1 } deve essere '{ $literal_2 }' o '{ $literal_3 }'.

journal.cli.sol.call.activities.span.absent = attività non trovata: { $span_id }

journal.cli.sol.call.activities.span.missing = Errore: argomento mancante '{ $literal_1 }'.

journal.cli.sol.call.activities.stdin.json = Errore: { $literal_1 } non valido su stdin: { $error }

journal.cli.sol.call.activities.stdin.object = Errore: atteso un oggetto { $literal_1 } su stdin.

journal.cli.sol.call.activities.title.option = Errore: { $literal_1 } è obbligatorio.

journal.cli.sol.call.activities.title.required = Errore: il titolo è obbligatorio.

journal.cli.sol.call.body.coverage = copertura: { $start } a { $end }

journal.cli.sol.call.body.day = giorno: { $day }

journal.cli.sol.call.body.days = giorni: { $days }

journal.cli.sol.call.body.entries = elementi: { $count }

journal.cli.sol.call.body.imports = importazioni: { $imported }

journal.cli.sol.call.body.range = finestra: { $start } a { $end }

journal.cli.sol.call.body.sources = fonti: { $sources }

journal.cli.sol.call.body.unreadable = non è stato possibile leggere la risposta dell'app del corpo.

journal.cli.sol.call.facets.accepted = Accettata la candidata di facetta '{ $name_key }' come '{ $facet }'.

journal.cli.sol.call.facets.action = risultato di { $action } per '{ $name_key }': { $result }

journal.cli.sol.call.facets.already.accepted = Candidata di facetta '{ $name_key }' già accettata.

journal.cli.sol.call.facets.already.dismissed = Candidata di facetta '{ $name_key }' già ignorata.

journal.cli.sol.call.facets.candidate = { $name }  [{ $status }]  count={ $count }  last={ $day }

journal.cli.sol.call.facets.dismissed = Ignorata la candidata di facetta '{ $name_key }'.

journal.cli.sol.call.facets.empty = Nessuna candidata di facetta trovata.

journal.cli.sol.call.thinking.attestation.expires = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.observed = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.reason = { $field_key }: { $value }

journal.cli.sol.call.thinking.attestation.state = { $field_key }: { $value }

journal.cli.sol.call.thinking.continuing = l'operazione continua lato server; { $literal_1 } ne mostra l'avanzamento.

journal.cli.sol.call.thinking.enabled = { $field_key }: { $value }

journal.cli.sol.call.thinking.ended = l'operazione è finita senza attivare l'elaborazione confidenziale; controlla { $literal_1 }.

journal.cli.sol.call.thinking.lane = corsia attiva: { $lane }

journal.cli.sol.call.thinking.next = avanti: { $literal_1 }

journal.cli.sol.call.thinking.not.ready = non pronto

journal.cli.sol.call.thinking.operation = operazione: { $operation_phase }

journal.cli.sol.call.thinking.portal = continua nel browser { $arrow } { $url }

journal.cli.sol.call.thinking.provenance = { $field_key }: { $value }

journal.cli.sol.call.thinking.ready = pronto

journal.cli.sol.call.thinking.subscribe = { $field_key }: { $url }

journal.cli.sol.call.transcripts.actionable = { $marker } azionabile: { $literal_1 }

journal.cli.sol.call.transcripts.day.totals = { $day }  trascrizioni:{ $n_transcripts } percetti:{ $n_percepts }

journal.cli.sol.call.transcripts.full.raw = Errore: Non puoi usare { $literal_1 } e { $literal_2 } insieme.

journal.cli.sol.call.transcripts.heading.percepts = Percetti:

journal.cli.sol.call.transcripts.heading.transcripts = Trascrizioni:

journal.cli.sol.call.transcripts.length.integer = Errore: l'opzione { $literal_1 } richiede un numero intero.

journal.cli.sol.call.transcripts.max.integer = Errore: l'opzione { $literal_1 } richiede un numero intero.

journal.cli.sol.call.transcripts.month.empty = Nessun dato per { $month }.

journal.cli.sol.call.transcripts.none = (nessuno)

journal.cli.sol.call.transcripts.pending = { $count ->
    [one] { $indent }{ $start } - { $end } ({ $display_count } segmento in attesa a { $starts })
    [many] { $indent }{ $start } - { $end } ({ $display_count } di segmenti in attesa a { $starts })
   *[other] { $indent }{ $start } - { $end } ({ $display_count } segmenti in attesa a { $starts })
    }

journal.cli.sol.call.transcripts.range.flags = Errore: Non puoi mescolare { $literal_1 }, { $literal_2 } e { $literal_3 }/{ $literal_4 }.

journal.cli.sol.call.transcripts.segments.empty = Nessun segmento.

journal.cli.sol.call.transcripts.source.flags = Errore: Non puoi mescolare { $literal_1 }/{ $literal_2 } con i flag di fonte singoli.

journal.cli.sol.call.transcripts.speakers.heading = Parlanti per { $day }/{ $stream }/{ $segment }:

journal.cli.sol.call.transcripts.start.invalid = Errore: l'ora di inizio deve essere { $literal_1 }.

journal.cli.sol.call.transcripts.tag.owner = { $marker } non azionabile: { $literal_1 }

journal.cli.sol.call.transcripts.total.days = { $count ->
    [one] Totale: { $display_count } giorno con dati
    [many] Totale: { $display_count } di giorni con dati
   *[other] Totale: { $display_count } giorni con dati
    }

journal.cli.sol.call.transcripts.truncated = [troncato: { $total } byte in totale, { $literal_1 } { $maximum }]

journal.cli.sol.call.transcripts.unknown.voice = voce sconosciuta

journal.cli.sol.call.typer.error = Errore

journal.cli.sol.call.typer.missing.argument = Argomento mancante '{ $name }'.

journal.cli.sol.call.typer.missing.option = Opzione mancante '{ $option }'.

journal.cli.sol.call.typer.try = Prova '{ $command } { $literal_1 }' per l'aiuto.

journal.cli.sol.entities.accept.merge.candidate.accept.result.for = risultato dell'accettazione per { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.accept.merge.candidate.accepted.merge.candidate = Candidato all'unione accettato: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.candidate.already.accepted = Candidato all'unione già accettato: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.accept.merge.candidate.merge.is.permanent = L'unione { $merge_id } è permanente.

journal.cli.sol.entities.aka.added.alias.to = Alias '{ $aka_value }' aggiunto a '{ $resolved_name }'.

journal.cli.sol.entities.aka.alias.already.exists.for = Alias '{ $aka_value }' esiste già per '{ $resolved_name }'.

journal.cli.sol.entities.aka.alias.is.the.first.word = Alias '{ $aka_value }' è la prima parola di '{ $resolved_name }' (saltato).

journal.cli.sol.entities.ambiguities.error.status.must.be.open = Errore: { $option_status } deve essere open o resolved.

journal.cli.sol.entities.ambiguities.from = { $spacing_2 }da: { $sources }

journal.cli.sol.entities.ambiguities.no.entity.ambiguities.found = Nessuna ambiguità di entità trovata.

journal.cli.sol.entities.ambiguities.scope.tier = { $ambiguity_id }{ $spacing_2 }{ $name }{ $spacing_2 }[{ $status }]{ $spacing_2 }scope={ $scope }{ $spacing_2 }tier={ $tier }

journal.cli.sol.entities.ambiguities.score = { $spacing_2 }- { $candidate } score={ $score }

journal.cli.sol.entities.attach.entity.already.attached = Entità '{ $entity }' già associata.

journal.cli.sol.entities.attach.entity.attached = Entità '{ $entity }' associata.

journal.cli.sol.entities.busy = non è stato possibile aggiornare quell'entità ora perché era occupata. riprova tra un momento.

journal.cli.sol.entities.delete.approval = L'eliminazione sposta l'entità e i suoi collegamenti di facetta nel cestino; { $option_erase } la cancella per sempre. Quando approvi, eseguilo di nuovo con { $option_yes }.

journal.cli.sol.entities.delete.entity.erased = Cancellata '{ $entity_id }'.

journal.cli.sol.entities.delete.entity.error.wasn.t.deleted = Errore: '{ $entity_id }' non è stata eliminata: { $reason }.

journal.cli.sol.entities.delete.failed.without.reason.error.wasn.t.deleted.it = Errore: '{ $entity_id }' non è stata eliminata: non è stata eliminata.

journal.cli.sol.entities.detect.entity.detected.for = Entità '{ $entity_name }' rilevata per { $day }.

journal.cli.sol.entities.dismiss.merge.candidate.dismiss.result.for = risultato di ignora per { $source_slug } -> { $target_slug }: { $status }

journal.cli.sol.entities.dismiss.merge.candidate.dismissed.merge.candidate = Ignorato il candidato all'unione: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.dismiss.merge.candidate.merge.candidate.already.dismissed = Candidato all'unione già ignorato: { $source_slug } -> { $target_slug }

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found = Errore: Entità '{ $query }' non trovata.

journal.cli.sol.entities.edge.resolution.error.error.entity.not.found.did = Errore: Entità '{ $query }' non trovata. Intendevi: { $labels }

journal.cli.sol.entities.edge.resolution.without.query.empty.error.entity.entity.not.found = Errore: Entità 'entity' non trovata.

journal.cli.sol.entities.edge.resolution.without.query.suggestions.error.entity.entity.not.found = Errore: Entità 'entity' non trovata. Intendevi: { $labels }

journal.cli.sol.entities.edges.unavailable = Le connessioni non sono disponibili ora.

journal.cli.sol.entities.entity.error.error.entity.is.hidden = Errore: Entità '{ $name }' è nascosta.

journal.cli.sol.entities.entity.error.error.entity.not.found = Errore: Entità '{ $name }' non trovata.

journal.cli.sol.entities.entity.error.error.invalid.entity.type = Errore: Tipo di entità non valido '{ $entity_type }'.

journal.cli.sol.entities.entity.versions.merge = { $spacing_2 }merge={ $merge_id } ({ $description })

journal.cli.sol.entities.entity.versions.no.identity.history.for = Nessuna cronologia dell'identità per { $entity }.

journal.cli.sol.entities.entity.versions.unknown.no.identity.history.for.entity = Nessuna cronologia dell'identità per l'entità.

journal.cli.sol.entities.error.without.name.hidden.error.entity.entity.is.hidden = Errore: Entità 'entity' è nascosta.

journal.cli.sol.entities.evidence.default.day.unknown.day = unknown-day

journal.cli.sol.entities.evidence.kind.missing.unknown = sconosciuto

journal.cli.sol.entities.history.all.evidence.rows.for = { $quantity ->
    [one] { $total } riga di prova per { $entity_id } <-> { $peer }:
    [many] { $total } di righe di prova per { $entity_id } <-> { $peer }:
   *[other] { $total } righe di prova per { $entity_id } <-> { $peer }:
    }

journal.cli.sol.entities.history.no.recorded.connection.history.between = Nessuna cronologia di connessione registrata tra { $entity_id } e { $peer }.

journal.cli.sol.entities.history.no.visible.rows = { $quantity ->
    [one] { $total } riga di prova per { $entity_id } <-> { $peer } (mostra 0):
    [many] { $total } di righe di prova per { $entity_id } <-> { $peer } (mostra 0):
   *[other] { $total } righe di prova per { $entity_id } <-> { $peer } (mostra 0):
    }

journal.cli.sol.entities.history.range.evidence.rows.for.showing = { $quantity ->
    [one] { $total } riga di prova per { $entity_id } <-> { $peer } (mostra { $first }-{ $last }):
    [many] { $total } di righe di prova per { $entity_id } <-> { $peer } (mostra { $first }-{ $last }):
   *[other] { $total } righe di prova per { $entity_id } <-> { $peer } (mostra { $first }-{ $last }):
    }

journal.cli.sol.entities.list.attached.attached.entities = { $quantity ->
    [one] { $count } entità associata:
    [many] { $count } di entità associate:
   *[other] { $count } entità associate:
    }

journal.cli.sol.entities.list.detected.detected.for.entities = { $quantity ->
    [one] { $count } entità rilevata per { $day }:
    [many] { $count } rilevate per { $day } entità:
   *[other] { $count } rilevate per { $day } entità:
    }

journal.cli.sol.entities.list.hidden.hidden = { $spacing_2 }- { $name } nascosta ({ $entity_type }): { $description }

journal.cli.sol.entities.list.no.entities.found = Nessuna entità trovata.

journal.cli.sol.entities.list.visible.row = { $spacing_2 }- { $name } ({ $entity_type }): { $description }

journal.cli.sol.entities.merge.approval = Un'unione non si può annullare. Mostra l'anteprima senza { $option_commit }, poi, quando approvi, eseguila di nuovo con { $option_commit } { $option_yes }.

journal.cli.sol.entities.merge.candidates.facet.detections.needs.last = { $source } -> { $target }{ $spacing_2 }[{ $status }]{ $spacing_2 }facet={ $facet }{ $spacing_2 }detections={ $detections }{ $spacing_2 }needs={ $needs }{ $spacing_2 }last={ $last }

journal.cli.sol.entities.merge.candidates.no.merge.candidates.found = Nessun candidato all'unione trovato.

journal.cli.sol.entities.merge.failure.default.error.operation.failed = Errore: operazione non riuscita

journal.cli.sol.entities.merge.preview.aliases.added = { $spacing_2 }alias aggiunti: { $aliases }

journal.cli.sol.entities.merge.preview.emails.added = { $spacing_2 }email aggiunte: { $emails }

journal.cli.sol.entities.merge.preview.facet.links.moved.merged = { $spacing_2 }collegamenti di facetta: { $moved } spostati, { $merged } uniti

journal.cli.sol.entities.merge.preview.merge.preview = Anteprima dell'unione:

journal.cli.sol.entities.merge.preview.observations.moved = { $spacing_2 }osservazioni spostate: { $observations }

journal.cli.sol.entities.merge.preview.segment.update.errors = { $spacing_2 }errori di aggiornamento dei segmenti: { $errors }

journal.cli.sol.entities.merge.preview.speaker.labels.updated.labels.corrections = { $quantity ->
    [one] { $corrections_quantity ->
        [one] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } etichetta, { $corrections } correzione
        [many] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } etichetta, { $corrections } di correzioni
       *[other] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } etichetta, { $corrections } correzioni
    }
    [many] { $corrections_quantity ->
        [one] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } di etichette, { $corrections } correzione
        [many] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } di etichette, { $corrections } di correzioni
       *[other] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } di etichette, { $corrections } correzioni
    }
   *[other] { $corrections_quantity ->
        [one] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } etichette, { $corrections } correzione
        [many] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } etichette, { $corrections } di correzioni
       *[other] { $spacing_2 }etichette dei parlanti aggiornate: { $labels } etichette, { $corrections } correzioni
    }
}

journal.cli.sol.entities.merge.preview.this.preview.covers.alias.and = { $spacing_2 }Questa anteprima copre le aggiunte di alias e di email; l'unione può anche cambiare i collegamenti di facetta, le note, le etichette dei parlanti e i campioni di voce.

journal.cli.sol.entities.merge.preview.voice.samples.moved.added.total = { $spacing_2 }campioni di voce spostati: { $added } aggiunti, { $total } totale

journal.cli.sol.entities.move.entity.error.facet.from.does.not = Errore: La facetta '{ $from_facet }' ({ $option_from }) non esiste.

journal.cli.sol.entities.move.entity.error.facet.to.does.not = Errore: La facetta '{ $to_facet }' ({ $option_to }) non esiste.

journal.cli.sol.entities.move.entity.moved.entity.from.to = Spostata l'entità '{ $entity_name }' da '{ $from_facet }' a '{ $to_facet }'.

journal.cli.sol.entities.neighbor.count = count={ $count }

journal.cli.sol.entities.neighbor.kinds = kinds={ $kinds }

journal.cli.sol.entities.neighbor.score = score={ $score }

journal.cli.sol.entities.network.all.recorded.connections.for = { $quantity ->
    [one] { $total } connessione registrata per { $entity_id }:
    [many] { $total } di connessioni registrate per { $entity_id }:
   *[other] { $total } connessioni registrate per { $entity_id }:
}

journal.cli.sol.entities.network.directed.both.directed.out.in = directed=out:{ $out_count }, in:{ $in_count }

journal.cli.sol.entities.network.directed.in.directed.in = directed=in:{ $in_count }

journal.cli.sol.entities.network.directed.out.directed.out = directed=out:{ $out_count }

journal.cli.sol.entities.network.limited.recorded.connections.for.showing = { $quantity ->
    [one] { $total } connessione registrata per { $entity_id } (in mostra { $shown }):
    [many] { $total } di connessioni registrate per { $entity_id } (in mostra { $shown }):
   *[other] { $total } connessioni registrate per { $entity_id } (in mostra { $shown }):
}

journal.cli.sol.entities.network.no.recorded.connections.for = Nessuna connessione registrata per { $entity_id }.

journal.cli.sol.entities.network.seen.range.seen = seen={ $first }..{ $last }

journal.cli.sol.entities.network.seen.seen = seen={ $day }

journal.cli.sol.entities.observation.error.couldn.t.read.the.moments = non è stato possibile leggere i momenti salvati per '{ $name }': la riga { $line_str } del file non è valida. non è stato cambiato niente.

journal.cli.sol.entities.observations.newest.moments.for.in.newest.first = { $quantity ->
    [one] { $total } momento per '{ $name }' in '{ $facet }', il più recente prima:
    [many] { $total } di momenti per '{ $name }' in '{ $facet }', i più recenti prima:
   *[other] { $total } momenti per '{ $name }' in '{ $facet }', i più recenti prima:
}

journal.cli.sol.entities.observations.nothing.with.yet = niente con { $resolved_name } ancora.

journal.cli.sol.entities.observations.oldest.moments.for.in.oldest.first = { $quantity ->
    [one] { $total } momento per '{ $name }' in '{ $facet }', il più vecchio prima:
    [many] { $total } di momenti per '{ $name }' in '{ $facet }', i più vecchi prima:
   *[other] { $total } momenti per '{ $name }' in '{ $facet }', i più vecchi prima:
}

journal.cli.sol.entities.observations.partial.most.recent.showing.the.most.recent.of = sono in mostra i { $shown } più recenti di { $total } momenti per '{ $name }'. aggiungi { $option_all } per vederli tutti.

journal.cli.sol.entities.observations.partial.oldest.showing.the.oldest.of.moments = sono in mostra i { $shown } più vecchi di { $total } momenti per '{ $name }'. aggiungi { $option_all } per vederli tutti.

journal.cli.sol.entities.observe.added.added.moments.for.in = { $quantity ->
    [one] aggiunto. { $count } momento per '{ $name }' in '{ $facet }'.
    [many] aggiunti. { $count } di momenti per '{ $name }' in '{ $facet }'.
   *[other] aggiunti. { $count } momenti per '{ $name }' in '{ $facet }'.
}

journal.cli.sol.entities.observe.your.journal.already.holds.that = il tuo diario tiene già quel momento per '{ $resolved_name }'. non è stato aggiunto niente.

journal.cli.sol.entities.overview.all.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
        [one] Panoramica della rete: { $edges } arco su { $entities } entità:
        [many] Panoramica della rete: { $edges } arco su { $entities } di entità:
       *[other] Panoramica della rete: { $edges } arco su { $entities } entità:
    }
    [many] { $entities_quantity ->
        [one] Panoramica della rete: { $edges } di archi su { $entities } entità:
        [many] Panoramica della rete: { $edges } di archi su { $entities } di entità:
       *[other] Panoramica della rete: { $edges } di archi su { $entities } entità:
    }
   *[other] { $entities_quantity ->
        [one] Panoramica della rete: { $edges } archi su { $entities } entità:
        [many] Panoramica della rete: { $edges } archi su { $entities } di entità:
       *[other] Panoramica della rete: { $edges } archi su { $entities } entità:
    }
}

journal.cli.sol.entities.overview.kinds = Tipi: { $kinds }

journal.cli.sol.entities.overview.limited.network.overview.edges.across.entities = { $quantity ->
    [one] { $entities_quantity ->
        [one] Panoramica della rete: { $edges } arco su { $entities } entità (in mostra { $shown }):
        [many] Panoramica della rete: { $edges } arco su { $entities } di entità (in mostra { $shown }):
       *[other] Panoramica della rete: { $edges } arco su { $entities } entità (in mostra { $shown }):
    }
    [many] { $entities_quantity ->
        [one] Panoramica della rete: { $edges } di archi su { $entities } entità (in mostra { $shown }):
        [many] Panoramica della rete: { $edges } di archi su { $entities } di entità (in mostra { $shown }):
       *[other] Panoramica della rete: { $edges } di archi su { $entities } entità (in mostra { $shown }):
    }
   *[other] { $entities_quantity ->
        [one] Panoramica della rete: { $edges } archi su { $entities } entità (in mostra { $shown }):
        [many] Panoramica della rete: { $edges } archi su { $entities } di entità (in mostra { $shown }):
       *[other] Panoramica della rete: { $edges } archi su { $entities } entità (in mostra { $shown }):
    }
}

journal.cli.sol.entities.overview.no.recorded.connections.in.the = Nessuna connessione registrata nell'indice degli archi.

journal.cli.sol.entities.overview.type = type={ $entity_type }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.recorded = candidato all'unione registrato: { $source } -> { $target }

journal.cli.sol.entities.record.merge.candidate.merge.candidate.updated.status = candidato all'unione aggiornato: { $source } -> { $target } (stato: { $status })

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.member = Errore: manca l'opzione obbligatoria { $option_member_id }

journal.cli.sol.entities.resolve.ambiguity.group.error.missing.required.option.revision = Errore: manca l'opzione obbligatoria { $option_revision }

journal.cli.sol.entities.resolve.ambiguity.group.refusing.to.resolve.this.ambiguity = Senza { $option_yes } questo gruppo di ambiguità non viene risolto.

journal.cli.sol.entities.resolve.ambiguity.group.resolved.ambiguities.to = { $quantity ->
    [one] Risolta { $count } ambiguità verso { $entity_id }.
    [many] Risolte { $count } di ambiguità verso { $entity_id }.
   *[other] Risolte { $count } ambiguità verso { $entity_id }.
}

journal.cli.sol.entities.resolve.ambiguity.refusing.to.resolve.this.ambiguity = Senza { $option_yes } questa ambiguità non viene risolta.

journal.cli.sol.entities.resolve.ambiguity.resolved.to.at = Risoluzione di { $ambiguity_id } verso { $entity_id } a { $resolved_at }.

journal.cli.sol.entities.resolve.day.error.day.is.required.pass = Errore: il giorno è obbligatorio (passalo come argomento o imposta { $literal_sol_day }).

journal.cli.sol.entities.resolve.error.error.entity.is.hidden = Errore: L'entità '{ $blocked_name }' è nascosta.

journal.cli.sol.entities.resolve.error.error.entity.not.found.did = Errore: L'entità '{ $entity }' non è stata trovata. Intendevi: { $names }

journal.cli.sol.entities.resolve.error.error.entity.not.found.in = Errore: L'entità '{ $entity }' non è stata trovata nella facetta '{ $facet }'.

journal.cli.sol.entities.resolve.facet.error.facet.is.required.pass = Errore: la facetta è obbligatoria (passala come argomento o imposta { $literal_sol_facet }).

journal.cli.sol.entities.restore.review.error.missing.required.option.ambiguity = Errore: manca l'opzione obbligatoria { $option_ambiguity_id }

journal.cli.sol.entities.restore.review.error.missing.required.option.facet = Errore: manca l'opzione obbligatoria { $option_facet }

journal.cli.sol.entities.restore.review.error.missing.required.option.source = Errore: manca l'opzione obbligatoria { $option_source_slug }

journal.cli.sol.entities.restore.review.error.missing.required.option.target = Errore: manca l'opzione obbligatoria { $option_target_slug }

journal.cli.sol.entities.restore.review.error.target.must.be.ambiguity = Errore: { $literal_target } deve essere ambiguity o merge_candidate

journal.cli.sol.entities.restore.review.refusing.to.restore.review.item = Senza { $option_yes } l'elemento di revisione non viene ripristinato.

journal.cli.sol.entities.restore.review.restored.review.item = Ripristinato l'elemento di revisione { $target }.

journal.cli.sol.entities.restore.version.refusing.to.restore.this.identity = Senza { $option_yes } questa versione di identità non viene ripristinata.

journal.cli.sol.entities.restore.version.restored.from.new.history.version = Ripristino di { $entity_id } da { $version_id }; nuova versione della cronologia { $version }.

journal.cli.sol.entities.review.review.items.ambiguity.card.s = { $quantity ->
    [one] { $ambiguities_quantity ->
        [one] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
        [many] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
       *[other] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } scheda di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
    }
    [many] { $ambiguities_quantity ->
        [one] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
        [many] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
       *[other] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } di schede di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
    }
   *[other] { $ambiguities_quantity ->
        [one] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } ambiguità messa da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
        [many] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } di ambiguità messe da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
       *[other] { $candidates_quantity ->
            [one] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } candidato all'unione messo da parte.
            [many] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } di candidati all'unione messi da parte.
           *[other] Elementi di revisione: { $cards } scheda(e) di ambiguità, { $ambiguities } ambiguità(i) messa da parte, { $candidates } candidato(i) all'unione messo da parte.
        }
    }
}

journal.cli.sol.entities.search.activity.unreadable = L'attività delle entità rilevate non è leggibile. Esegui `{ $command }`, ripara il registro segnalato e riprova.

journal.cli.sol.entities.search.entities = { $quantity ->
    [one] { $count } entità:
    [many] { $count } di entità:
   *[other] { $count } entità:
}

journal.cli.sol.entities.search.facets = { $spacing_4 }facette: { $facets }

journal.cli.sol.entities.search.index.busy = La ricerca delle entità non è disponibile mentre l'indicizzazione è in corso. Riprova quando finisce.

journal.cli.sol.entities.search.index.stale = L'indice di ricerca delle entità non è aggiornato. Esegui `{ $command }` e riprova.

journal.cli.sol.entities.search.index.unavailable = L'indice di ricerca delle entità non è disponibile. Esegui `{ $command }` e riprova.

journal.cli.sol.entities.sweep.review.completed.entity.review.sweep = Completata la passata di revisione delle entità.

journal.cli.sol.entities.sweep.review.refusing.to.run.review.sweep = Senza { $option_yes } la passata di revisione non viene eseguita.

journal.cli.sol.entities.update.entity.updated = Entità '{ $resolved_name }' aggiornata.

journal.cli.sol.entities.update.entity.updated.for = Entità '{ $entity }' aggiornata per { $day }.

journal.cli.sol.health.activities.count = { $space_2 }{ $activities_count }: { $value }

journal.cli.sol.health.activities.user.edited = { $space_2 }{ $activities_user_edited }: { $value }

journal.cli.sol.health.activities.with.participation = { $space_2 }{ $activities_with_participation }: { $value }

journal.cli.sol.health.activities.with.story = { $space_2 }{ $activities_with_story }: { $value }

journal.cli.sol.health.argument.unexpected = Errore: argomento imprevisto { $token }.

journal.cli.sol.health.backlog.days = { $days_quantity ->
    [one] { $space_3 }nell'arco di { $days } giorno
    [many] { $space_3 }nell'arco di { $days } di giorni
   *[other] { $space_3 }nell'arco di { $days } giorni
}

journal.cli.sol.health.backlog.days.incomplete = { $days_quantity ->
    [one] { $space_3 }nell'arco di { $days } giorno (stato incompleto)
    [many] { $space_3 }nell'arco di { $days } di giorni (stato incompleto)
   *[other] { $space_3 }nell'arco di { $days } giorni (stato incompleto)
}

journal.cli.sol.health.backlog.media.days = { $media_quantity ->
    [one] { $days_quantity ->
        [one] { $space_2 }{ $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
        [many] { $space_2 }{ $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
       *[other] { $space_2 }{ $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
    }
    [many] { $days_quantity ->
        [one] { $space_2 }{ $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
        [many] { $space_2 }{ $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
       *[other] { $space_2 }{ $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
    }
   *[other] { $days_quantity ->
        [one] { $space_2 }{ $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
        [many] { $space_2 }{ $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
       *[other] { $space_2 }{ $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
    }
}

journal.cli.sol.health.backlog.media.days.incomplete = { $media_quantity ->
    [one] { $days_quantity ->
        [one] { $space_2 }almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
        [many] { $space_2 }almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
       *[other] { $space_2 }almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
    }
    [many] { $days_quantity ->
        [one] { $space_2 }almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
        [many] { $space_2 }almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
       *[other] { $space_2 }almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
    }
   *[other] { $days_quantity ->
        [one] { $space_2 }almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
        [many] { $space_2 }almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
       *[other] { $space_2 }almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
    }
}

journal.cli.sol.health.backlog.thinking.days = { $thinking_quantity ->
    [one] { $days_quantity ->
        [one] { $space_2 }{ $thinking } segmento in attesa di ragionamento nell'arco di { $days } giorno
        [many] { $space_2 }{ $thinking } segmento in attesa di ragionamento nell'arco di { $days } di giorni
       *[other] { $space_2 }{ $thinking } segmento in attesa di ragionamento nell'arco di { $days } giorni
    }
    [many] { $days_quantity ->
        [one] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento nell'arco di { $days } giorno
        [many] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento nell'arco di { $days } di giorni
       *[other] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento nell'arco di { $days } giorni
    }
   *[other] { $days_quantity ->
        [one] { $space_2 }{ $thinking } segmenti in attesa di ragionamento nell'arco di { $days } giorno
        [many] { $space_2 }{ $thinking } segmenti in attesa di ragionamento nell'arco di { $days } di giorni
       *[other] { $space_2 }{ $thinking } segmenti in attesa di ragionamento nell'arco di { $days } giorni
    }
}

journal.cli.sol.health.backlog.thinking.days.incomplete = { $thinking_quantity ->
    [one] { $days_quantity ->
        [one] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento nell'arco di { $days } giorno (stato incompleto)
        [many] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento nell'arco di { $days } di giorni (stato incompleto)
       *[other] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento nell'arco di { $days } giorni (stato incompleto)
    }
    [many] { $days_quantity ->
        [one] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento nell'arco di { $days } giorno (stato incompleto)
        [many] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento nell'arco di { $days } di giorni (stato incompleto)
       *[other] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento nell'arco di { $days } giorni (stato incompleto)
    }
   *[other] { $days_quantity ->
        [one] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento nell'arco di { $days } giorno (stato incompleto)
        [many] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento nell'arco di { $days } di giorni (stato incompleto)
       *[other] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento nell'arco di { $days } giorni (stato incompleto)
    }
}

journal.cli.sol.health.backlog.thinking.media.days = { $thinking_quantity ->
    [one] { $media_quantity ->
        [one] { $days_quantity ->
            [one] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
        [many] { $days_quantity ->
            [one] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
       *[other] { $days_quantity ->
            [one] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } segmento in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
    }
    [many] { $media_quantity ->
        [one] { $days_quantity ->
            [one] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
        [many] { $days_quantity ->
            [one] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
       *[other] { $days_quantity ->
            [one] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } di segmenti in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
    }
   *[other] { $media_quantity ->
        [one] { $days_quantity ->
            [one] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
        [many] { $days_quantity ->
            [one] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
       *[other] { $days_quantity ->
            [one] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno
            [many] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni
           *[other] { $space_2 }{ $thinking } segmenti in attesa di ragionamento, { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni
        }
    }
}

journal.cli.sol.health.backlog.thinking.media.days.incomplete = { $thinking_quantity ->
    [one] { $media_quantity ->
        [one] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
        [many] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
       *[other] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } segmento in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
    }
    [many] { $media_quantity ->
        [one] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
        [many] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
       *[other] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } di segmenti in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
    }
   *[other] { $media_quantity ->
        [one] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } segmento in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
        [many] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } di segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
       *[other] { $days_quantity ->
            [one] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorno (stato incompleto)
            [many] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } di giorni (stato incompleto)
           *[other] { $space_2 }almeno { $thinking } segmenti in attesa di ragionamento, almeno { $media } segmenti in attesa di elaborazione dei contenuti multimediali nell'arco di { $days } giorni (stato incompleto)
        }
    }
}

journal.cli.sol.health.backlog.unavailable = { $space_2 }Stato dell'analisi dei segmenti non disponibile

journal.cli.sol.health.consumer = Segnali di consumo

journal.cli.sol.health.coverage.ratio = { $space_2 }{ $coverage_ratio }: { $value }

journal.cli.sol.health.facets.silent.past.day = { $space_2 }{ $facets_silent_24h }: { $value }

journal.cli.sol.health.facets.with.recent.capture = { $space_2 }{ $facets_with_recent_capture }: { $value }

journal.cli.sol.health.hours.total = { $space_2 }{ $hours_total }: { $value }

journal.cli.sol.health.hours.with.capture = { $space_2 }{ $hours_with_capture }: { $value }

journal.cli.sol.health.index = { $space_2 }search_index: { $state } (non aggiornato { $stale }, mancante { $missing }, orfano { $orphaned }, non riuscito { $failed })

journal.cli.sol.health.index.activity.at = { $space_2 }{ $index_activity_at }: { $value }

journal.cli.sol.health.intake = Ingresso

journal.cli.sol.health.last.segment.at = { $space_2 }{ $last_segment_at }: { $value }

journal.cli.sol.health.none = { $space_2 }niente

journal.cli.sol.health.notes = Note

journal.cli.sol.health.profile.entities.total = { $space_2 }{ $profile_entities_total }: { $value }

journal.cli.sol.health.range = Intervallo: { $start } -> { $end }

journal.cli.sol.health.silent = Dettaglio delle facette silenziose

journal.cli.sol.health.synthesis = Sintesi

journal.cli.sol.health.talent.degraded.outputs.past.day = { $space_2 }{ $talent_degraded_outputs_24h }: { $value }

journal.cli.sol.health.talent.run.failures.past.day = { $space_2 }{ $talent_run_failures_24h }: { $value }

journal.cli.sol.help.apps = App ({ $invocation }):

journal.cli.sol.help.authority = Autorità: { $path }

journal.cli.sol.help.command.activities.create = Crea un nuovo registro sintetico di attività da flag di { $argv } o da { $json } su { $stdin }.

journal.cli.sol.help.command.activities.get = Recupera un registro di attività per ID.

journal.cli.sol.help.command.activities.list = Elenca i registri di attività per un giorno o un intervallo di giorni inclusivo.

journal.cli.sol.help.command.activities.mute = Nascondi un registro di attività senza eliminarlo.

journal.cli.sol.help.command.activities.unmute = Ripristina un registro di attività nascosto in precedenza.

journal.cli.sol.help.command.activities.update = Applica un flag { $argv } o una patch { $json } da { $stdin } a un registro di attività.

journal.cli.sol.help.command.awareness.imports = Leggi o aggiorna lo stato di tracciamento delle importazioni.

journal.cli.sol.help.command.awareness.log = Aggiungi un elemento al registro di consapevolezza del giorno.

journal.cli.sol.help.command.awareness.log.read = Leggi gli elementi dal registro di consapevolezza del giorno.

journal.cli.sol.help.command.awareness.status = Mostra lo stato di consapevolezza attuale.

journal.cli.sol.help.command.body.day = Mostra un giorno di dati di salute importati.

journal.cli.sol.help.command.body.status = Mostra lo stato dei dati di salute importati.

journal.cli.sol.help.command.body.window = Mostra il contesto di salute importato per una finestra di tempo.

journal.cli.sol.help.command.entities.accept.merge.candidate = Mostra l'anteprima o accetta un candidato all'unione di entità registrato.

journal.cli.sol.help.command.entities.aka = Aggiungi un alias a un'entità associata.

journal.cli.sol.help.command.entities.ambiguities = Elenca le ambiguità di risoluzione delle entità che si possono rivedere.

journal.cli.sol.help.command.entities.attach = Associa un'entità in modo permanente a una facetta.

journal.cli.sol.help.command.entities.delete = Sposta un'entità e i suoi collegamenti di facetta nel cestino; { $erase_flag } la cancella subito. Con { $erase_flag }, { $from_backups_flag } la rimuove anche dai tuoi backup e { $not_from_backups_flag } la lascia lì. Senza nessuno dei due, decide l'impostazione del cestino.

journal.cli.sol.help.command.entities.detect = Registra un'entità rilevata per un giorno in una facetta.

journal.cli.sol.help.command.entities.dismiss.merge.candidate = Ignora un candidato all'unione di entità registrato.

journal.cli.sol.help.command.entities.entity.history = Mostra le versioni di identità durevoli (non la cronologia delle relazioni).

journal.cli.sol.help.command.entities.history = Mostra la prova di connessione registrata per una coppia di entità.

journal.cli.sol.help.command.entities.list = Elenca le entità di una facetta.

journal.cli.sol.help.command.entities.merge = Pianifica o esegui un'unione di entità del diario.

journal.cli.sol.help.command.entities.merge.candidates = Elenca i candidati all'unione di entità registrati.

journal.cli.sol.help.command.entities.move = Sposta un'entità da una facetta a un'altra.

journal.cli.sol.help.command.entities.network = Mostra le connessioni registrate a un salto per un'entità.

journal.cli.sol.help.command.entities.observations = Elenca le osservazioni di un'entità associata.

journal.cli.sol.help.command.entities.observe = Aggiungi un'osservazione a un'entità associata.

journal.cli.sol.help.command.entities.overview = Mostra una panoramica globale delle connessioni registrate.

journal.cli.sol.help.command.entities.record.merge.candidate = Registra un'unione di entità proposta (variante di origine -> destinazione canonica).

journal.cli.sol.help.command.entities.resolve.ambiguity = Risolvi o sostituisci una scelta di ambiguità salvata.

journal.cli.sol.help.command.entities.resolve.ambiguity.group = Risolvi un gruppo di schede di ambiguità per una candidata di entità.

journal.cli.sol.help.command.entities.restore.review = Ripristina un elemento di revisione messo da parte o annulla una scelta automatica.

journal.cli.sol.help.command.entities.restore.version = Ripristina una versione di identità ordinaria.

journal.cli.sol.help.command.entities.review = Elenca le schede di revisione delle entità raggruppate e gli elementi messi da parte.

journal.cli.sol.help.command.entities.search = Cerca entità per testo, tipo, facetta o attività.

journal.cli.sol.help.command.entities.sweep.review = Esegui la passata del criterio di revisione delle entità.

journal.cli.sol.help.command.entities.update = Aggiorna la descrizione di un'entità.

journal.cli.sol.help.command.facets.accept = Accetta un candidato di revisione di facetta.

journal.cli.sol.help.command.facets.dismiss = Ignora un candidato di revisione di facetta.

journal.cli.sol.help.command.facets.list.candidates = Elenca i candidati di revisione di facetta registrati.

journal.cli.sol.help.command.health.for.range = Produci il rapporto di fiducia sui dati del diario per un intervallo di giorni inclusivo.

journal.cli.sol.help.command.health.full = Produci il rapporto di fiducia completo sui dati del diario per un giorno.

journal.cli.sol.help.command.health.pipeline = Involucro sottile intorno a { $pipeline_module }. Per i controlli di fiducia sui dati del diario usa `{ $summary_verb }` / `{ $full_verb }` / `{ $for_range_verb }`.

journal.cli.sol.help.command.health.summary = Riepiloga i segnali di fiducia sui dati del diario per un giorno.

journal.cli.sol.help.command.import.top.level = Importa contenuti multimediali attraverso il diario

journal.cli.sol.help.command.journal.agents = Elenca i risultati degli agenti disponibili per un giorno.

journal.cli.sol.help.command.journal.facet.create = Crea una nuova facetta.{ $line_break }{ $line_break }{ $consent_flag } registra nel registro di audit della facetta che l'hai approvato. Un{ $line_break }agente lo passa dopo che approvi una modifica che l'agente ha proposto, non per{ $line_break }una che hai chiesto tu. Non cambia ciò che viene creato.

journal.cli.sol.help.command.journal.facet.delete = Sposta una facetta e tutto ciò che contiene nel cestino, dove si può ripristinare finché non viene cancellata.{ $line_break }{ $line_break }{ $yes_flag } è obbligatorio; senza di esso il comando rifiuta. Ogni eliminazione viene registrata nel{ $line_break }registro delle azioni del diario come approvata da te, e { $consent_flag } è accettato ma non cambia{ $line_break }niente.

journal.cli.sol.help.command.journal.facet.mute = Silenzia una facetta (nascondila dagli elenchi predefiniti).

journal.cli.sol.help.command.journal.facet.rename = Rinomina una facetta.{ $line_break }{ $line_break }{ $consent_flag } registra nel registro di audit della facetta che l'hai approvato. Un{ $line_break }agente lo passa dopo che approvi una modifica che l'agente ha proposto, non per{ $line_break }una che hai chiesto tu. Non cambia la rinomina.

journal.cli.sol.help.command.journal.facet.show = Mostra il riepilogo della facetta.

journal.cli.sol.help.command.journal.facet.unmute = Togli il silenzio a una facetta (mostrala negli elenchi predefiniti).

journal.cli.sol.help.command.journal.facet.update = Aggiorna la configurazione della facetta.

journal.cli.sol.help.command.journal.facets = Elenca le facette.

journal.cli.sol.help.command.journal.import = Mostra i metadati completi di una singola importazione.

journal.cli.sol.help.command.journal.imports = Elenca le importazioni recenti con i metadati.

journal.cli.sol.help.command.journal.read = Leggi il contenuto completo di un risultato di agente.

journal.cli.sol.help.command.journal.retention.config = Mostra o aggiorna la configurazione di conservazione.

journal.cli.sol.help.command.journal.retention.list = Elenca i contenuti multimediali originali pronti per la rimozione.

journal.cli.sol.help.command.journal.search = Cerca nell'indice del diario.{ $line_break }{ $line_break }Usa 2-4 termini di contenuto invece di domande in linguaggio naturale; le parole di domanda{ $line_break }come { $example_terms } aggiungono rumore in questo indice di parole chiave/{ $bm25 }. Sintassi: { $or_operator } per{ $line_break }qualsiasi termine, frasi tra virgolette per il testo esatto e { $wildcard } per le corrispondenze di prefisso.{ $line_break }{ $n_flag }/{ $limit_flag } è il massimo di risultati per giorno. I risultati sono raggruppati per giorno,{ $line_break }prima i più recenti, fino a 20 giorni per pagina. { $offset_flag } salta i giorni. { $d_flag }/{ $day_flag } è un{ $line_break }giorno esatto e non si può combinare con { $day_from_flag } o { $day_to_flag }. Quando niente{ $line_break }corrisponde a ogni termine dentro gli altri filtri (facetta, agente, flusso, fascia di{ $line_break }tempo) in tutto il diario, la risposta è marcata { $relaxed } e quei{ $line_break }risultati sono corrispondenze più deboli; quel giudizio avviene prima del filtro sulla data, quindi un{ $line_break }giorno o intervallo vuoto non significa che l'intervallo non menzioni mai i termini. Allarga{ $line_break }togliendo termini, poi { $or_operator }, poi { $wildcard }. { $json_flag } non cambia l'uscita{ $line_break }perché è { $json } in ogni caso.{ $line_break }Leggi un risultato come ultimo indicizzato con `{ $command }`. Copia tutti e tre i campi dallo stesso risultato; non usare il suo id di visualizzazione come percorso di file.

journal.cli.sol.help.command.journal.storage.summary = Mostra il riepilogo dello spazio del diario.

journal.cli.sol.help.command.link.authorized.clients = Elenca ogni certificato client autorizzato: impronta, etichetta, ultima presenza (vista piatta).

journal.cli.sol.help.command.link.confirm = conferma la marca del diario per un link che la attende

journal.cli.sol.help.command.link.join = unisciti a un { $solstone } con un codice breve o un link di accoppiamento

journal.cli.sol.help.command.link.list = Stampa ogni dispositivo accoppiato con l'orario di ultima presenza.

journal.cli.sol.help.command.link.local.network.close = Chiudi il tuo diario ai dispositivi sulla tua rete.

journal.cli.sol.help.command.link.local.network.open = Apri il tuo diario ai dispositivi sulla tua rete.

journal.cli.sol.help.command.link.local.network.status = Mostra se il tuo diario è aperto ai dispositivi sulla tua rete.

journal.cli.sol.help.command.link.observer.pause = metti in pausa i dispositivi collegati (non ancora disponibile).

journal.cli.sol.help.command.link.pair = Avvia un link di accoppiamento, stampa le credenziali pronte per unirsi e, facoltativamente, attendi.

journal.cli.sol.help.command.link.private.link.disable = Disattiva la rete privata.

journal.cli.sol.help.command.link.private.link.setup = Configura la rete privata.

journal.cli.sol.help.command.link.private.link.status = Mostra lo stato della rete privata.

journal.cli.sol.help.command.link.serve = servi un diario accoppiato attraverso il ponte del link locale

journal.cli.sol.help.command.link.set.label = Imposta o cancella l'etichetta del titolare per un dispositivo accoppiato.

journal.cli.sol.help.command.link.status = Segnala l'iscrizione, lo stato di { $listen_ws }, il numero di tunnel attivi, l'endpoint del relay.

journal.cli.sol.help.command.link.top.level.status = mostra lo stato del link e la versione osservata del diario remoto

journal.cli.sol.help.command.link.unpair = Revoca un dispositivo accoppiato. La riconnessione successiva da quel dispositivo non riesce all'handshake { $tls }.

journal.cli.sol.help.command.moved.navigate = Spostamento in `{ $command }`.

journal.cli.sol.help.command.settings.convey.status = Mostra lo stato della rete di { $convey } e dell'URL del pannello.

journal.cli.sol.help.command.settings.identity.set = Aggiorna l'identità del titolare del diario.

journal.cli.sol.help.command.settings.identity.show = Mostra la configurazione dell'identità del diario.

journal.cli.sol.help.command.settings.keys.clear = Cancella una chiave API dalla configurazione del diario.

journal.cli.sol.help.command.settings.keys.set = Imposta una chiave API nella configurazione del diario.

journal.cli.sol.help.command.settings.keys.show = Mostra lo stato della chiave API configurata.

journal.cli.sol.help.command.settings.keys.validate = Verifica tutte le chiavi API configurate senza conservarle per impostazione predefinita.

journal.cli.sol.help.command.settings.processing.set = Imposta la modalità di elaborazione differita e la finestra di svuotamento.

journal.cli.sol.help.command.settings.processing.show = Mostra le impostazioni effettive dell'elaborazione differita.

journal.cli.sol.help.command.settings.show = Mostra un riepilogo delle impostazioni del diario.

journal.cli.sol.help.command.settings.transcribe.set.backend = Imposta il backend di trascrizione.

journal.cli.sol.help.command.settings.transcribe.show = Mostra la configurazione del backend di trascrizione.

journal.cli.sol.help.command.speakers.attribute.segment = Esegui l'attribuzione del parlante (Livelli 1-3) su un singolo segmento.{ $line_break }{ $line_break }Classifica ogni frase con il rilevamento di te, euristiche strutturali{ $line_break }e la corrispondenza acustica delle impronte vocali.  Facoltativamente scrive { $labels_file }{ $line_break }e accumula impronte vocali ad alta confidenza.

journal.cli.sol.help.command.speakers.backfill = Esegui l'attribuzione dei parlanti su tutti i segmenti con embedding.{ $line_break }{ $line_break }Elabora i segmenti a partire dal più vecchio per costruire le impronte vocali in modo progressivo.{ $line_break }Salta i segmenti che hanno già { $labels_file } (si può eseguire di nuovo senza rischi).

journal.cli.sol.help.command.speakers.backfill.last.seen = Riempi a posteriori { $last_seen_field } sulle righe di metadati delle impronte vocali esistenti.

journal.cli.sol.help.command.speakers.backfill.resume = Riprendi un'operazione di riempimento a posteriori dell'attribuzione dei parlanti interrotta o non riuscita.

journal.cli.sol.help.command.speakers.backfill.status = Mostra lo stato di un'operazione di riempimento a posteriori dell'attribuzione dei parlanti.

journal.cli.sol.help.command.speakers.bootstrap = Inizializza le impronte vocali dai segmenti con un solo parlante.{ $line_break }{ $line_break }Scorre tutto il diario per i segmenti in cui { $speakers_file } elenca esattamente{ $line_break }un parlante. In quei segmenti, tutti gli embedding non tuoi appartengono a quel{ $line_break }parlante. Li salva come impronte vocali usando il tuo centroide per{ $line_break }la sottrazione della tua voce.

journal.cli.sol.help.command.speakers.build.from.tags = Costruisci il centroide della tua voce dai tuoi tag manuali già convalidati.

journal.cli.sol.help.command.speakers.confirm.owner = Conferma il candidato della tua voce nel browser.{ $line_break }{ $line_break }{ $confirm_verb } nativo non salva un centroide. Rivedi e conferma i candidati nel browser su { $speakers_page }.

journal.cli.sol.help.command.speakers.correct = Correggi subito un'attribuzione di parlante.

journal.cli.sol.help.command.speakers.day.segments = Elenca i segmenti di revisione dei parlanti per un giorno.

journal.cli.sol.help.command.speakers.detect = Esegui il rilevamento del candidato della tua voce.

journal.cli.sol.help.command.speakers.discover = Scopri i parlanti sconosciuti che ricorrono tra i segmenti.

journal.cli.sol.help.command.speakers.dismiss.cluster = Ignora un cluster di scoperta attuale dalle schermate di elenco.

journal.cli.sol.help.command.speakers.dismissals = Elenca le esclusioni consolidate dei cluster di parlanti.

journal.cli.sol.help.command.speakers.identify = Identifica un cluster scoperto di parlanti sconosciuti.

journal.cli.sol.help.command.speakers.identify.operation = Mostra un'operazione di identificazione del parlante oscurata.

journal.cli.sol.help.command.speakers.identify.operations = Elenca le operazioni di identificazione del parlante oscurate.

journal.cli.sol.help.command.speakers.identify.undo = Annulla un'operazione di identificazione del parlante già applicata.

journal.cli.sol.help.command.speakers.keep.separate.list = Elenca le asserzioni consolidate per tenere i parlanti separati.

journal.cli.sol.help.command.speakers.link.import = Collega il nome di un partecipante di un'importazione come alias su un'entità esistente.

journal.cli.sol.help.command.speakers.merge.names = Unisci una variante del nome di un parlante in un'entità canonica.

journal.cli.sol.help.command.speakers.owner.ready = Controlla se il rilevamento della tua voce va mostrato a te.

journal.cli.sol.help.command.speakers.presence = Mostra chi c'era intorno a un cluster scoperto di voce sconosciuta.

journal.cli.sol.help.command.speakers.propagate.correction = Mostra l'anteprima o applica la propagazione della correzione nell'ambito.{ $line_break }{ $line_break }L'inversione usa lo stesso verbo con i due id dei parlanti scambiati.

journal.cli.sol.help.command.speakers.rebuild.owner = Ricostruisci il centroide della tua voce dai tuoi tag manuali attuali.

journal.cli.sol.help.command.speakers.reject.owner = Rifiuta la generazione mostrata del candidato della tua voce.

journal.cli.sol.help.command.speakers.repair = Ripara la contaminazione da parlanti non-{ $person_type } senza riapprendere da essa.{ $line_break }{ $line_break }Esamina identità e impronte vocali, rimuove la contaminazione delle impronte vocali non-{ $person_type },{ $line_break }e aggiorna le etichette dei segmenti con un'attribuzione nuova nello stato attuale.

journal.cli.sol.help.command.speakers.repair.resume = Riprendi un'operazione di riparazione dei parlanti interrotta o non riuscita.

journal.cli.sol.help.command.speakers.repair.status = Mostra lo stato di un'operazione di riparazione dei parlanti.

journal.cli.sol.help.command.speakers.resolve.names = Risolvi le varianti dei nomi dei parlanti con la similarità delle impronte vocali.{ $line_break }{ $line_break }Confronta i centroidi delle impronte vocali tra tutte le entità. Le coppie con similarità del{ $line_break }coseno > 0.90 sono segnalate come la stessa persona. Le varianti non ambigue{ $line_break }(il nome breve è la prima parola del nome completo) vengono unite in automatico aggiungendo il{ $line_break }nome breve come alias sull'entità canonica.

journal.cli.sol.help.command.speakers.seed.from.imports = Semina le impronte vocali dai segmenti di importazione con trascrizioni attribuite ai parlanti.{ $line_break }{ $line_break }Scorre i flussi di importazione per i segmenti che hanno sia { $transcript_file }{ $line_break }(con etichette dei parlanti) sia embedding audio. Associa ogni embedding a un parlante{ $line_break }tramite allineamento temporale, fa corrispondere i parlanti alle entità esistenti e salva{ $line_break }gli embedding come impronte vocali con salvaguardia contro la contaminazione della tua voce.

journal.cli.sol.help.command.speakers.sentences = Elenca gli id delle frasi per una fonte di revisione dei parlanti.

journal.cli.sol.help.command.speakers.status = Mostra lo stato del sottosistema dei parlanti come { $json }.

journal.cli.sol.help.command.speakers.suggest = Suggerisci opportunità di cura dei parlanti.

journal.cli.sol.help.command.speakers.tag.owner = Etichetta una frase come la tua voce.

journal.cli.sol.help.command.speakers.wipe = Sposta ogni file di voce dei parlanti nel diario nel cestino.{ $line_break }{ $line_break }Senza { $commit_flag }, stampa un rapporto di ciò che verrebbe spostato. Con{ $line_break }{ $commit_flag }, sposta gli NPZ degli embedding dei segmenti, le etichette/correzioni dei parlanti,{ $line_break }le impronte vocali per entità, i tuoi centroidi, l'istantanea del tuo candidato{ $line_break }e il pool dei candidati nel cestino, dove si possono recuperare finché{ $line_break }non vengono cancellati. { $erase_flag } li cancella subito, invece.{ $line_break }{ $line_break }Con { $erase_flag }, { $from_backups_flag } li rimuove anche dai tuoi backup e{ $line_break }{ $not_from_backups_flag } li lascia lì. Senza nessuno dei due, l'impostazione del cestino{ $line_break }decide.

journal.cli.sol.help.command.status.top.level = Mostra lo stato di rete del diario.

journal.cli.sol.help.command.thinking.clear.local.endpoint = Cancella l'endpoint del fornitore locale BYO.

journal.cli.sol.help.command.thinking.confidential.disable = Disattiva l'elaborazione confidenziale.

journal.cli.sol.help.command.thinking.confidential.enable = Attiva l'elaborazione confidenziale.

journal.cli.sol.help.command.thinking.confidential.recheck = Controlla di nuovo l'elaborazione confidenziale.

journal.cli.sol.help.command.thinking.confidential.status = Mostra lo stato dell'elaborazione confidenziale.

journal.cli.sol.help.command.thinking.keys.clear = Cancella una chiave IA dalla configurazione del diario.

journal.cli.sol.help.command.thinking.keys.set = Imposta una chiave IA nella configurazione del diario.

journal.cli.sol.help.command.thinking.keys.show = Mostra lo stato della chiave IA configurata.

journal.cli.sol.help.command.thinking.keys.validate = Valida le chiavi IA configurate.

journal.cli.sol.help.command.thinking.local.availability = Mostra la disponibilità del modello locale.

journal.cli.sol.help.command.thinking.local.bootstrap = Avvia la configurazione iniziale del modello locale.

journal.cli.sol.help.command.thinking.local.bootstrap.cancel = Annulla la configurazione iniziale del modello locale.

journal.cli.sol.help.command.thinking.local.bootstrap.status = Mostra lo stato della configurazione iniziale locale.

journal.cli.sol.help.command.thinking.local.models = Elenca i modelli locali.

journal.cli.sol.help.command.thinking.local.readiness = Mostra la prontezza del fornitore locale.

journal.cli.sol.help.command.thinking.local.status = Mostra lo stato del fornitore locale.

journal.cli.sol.help.command.thinking.providers.set.active = Imposta il fornitore e il modello usati per tutto il ragionamento.

journal.cli.sol.help.command.thinking.providers.show = Mostra la configurazione del fornitore.

journal.cli.sol.help.command.thinking.set.local.endpoint = Imposta l'endpoint del fornitore locale BYO.

journal.cli.sol.help.command.transcripts.read = Leggi il contenuto della trascrizione per un giorno, un segmento o un intervallo di tempo.

journal.cli.sol.help.command.transcripts.scan = Elenca gli intervalli di copertura della trascrizione per un giorno.

journal.cli.sol.help.command.transcripts.segments = Elenca i segmenti di registrazione per un giorno.

journal.cli.sol.help.command.transcripts.speakers = Elenca le righe di correzione dei parlanti per un segmento di trascrizione.

journal.cli.sol.help.command.transcripts.stats = Mostra i conteggi giornalieri di copertura della trascrizione per un mese.

journal.cli.sol.help.command.usage = Uso: { $syntax }

journal.cli.sol.help.computer = Questo computer

journal.cli.sol.help.journal = Il tuo diario

journal.cli.sol.help.operation = Operazione: { $operation }

journal.cli.sol.help.parameter = { $shape ->
    *[count] { $syntax } [conteggio]
    [multiple] { $syntax } [multiplo]
    [multiple_count] { $syntax } [multiplo, conteggio]
    [required] { $syntax } [obbligatorio]
    [required_count] { $syntax } [obbligatorio, conteggio]
    [required_multiple] { $syntax } [obbligatorio, multiplo]
    [required_multiple_count] { $syntax } [obbligatorio, multiplo, conteggio]
}

journal.cli.sol.help.parameter.default = { $shape ->
    *[plain] { $syntax } [predefinito: { $default_value }]
    [count] { $syntax } [predefinito: { $default_value }, conteggio]
    [multiple] { $syntax } [predefinito: { $default_value }, multiplo]
    [multiple_count] { $syntax } [predefinito: { $default_value }, multiplo, conteggio]
    [required] { $syntax } [obbligatorio, predefinito: { $default_value }]
    [required_count] { $syntax } [obbligatorio, predefinito: { $default_value }, conteggio]
    [required_multiple] { $syntax } [obbligatorio, predefinito: { $default_value }, multiplo]
    [required_multiple_count] { $syntax } [obbligatorio, predefinito: { $default_value }, multiplo, conteggio]
}

journal.cli.sol.help.parameter.default.flag = { $shape ->
    *[plain] { $syntax } [predefinito: { $default_value }, valore del flag: { $flag_value }]
    [count] { $syntax } [predefinito: { $default_value }, valore del flag: { $flag_value }, conteggio]
    [multiple] { $syntax } [predefinito: { $default_value }, valore del flag: { $flag_value }, multiplo]
    [multiple_count] { $syntax } [predefinito: { $default_value }, valore del flag: { $flag_value }, multiplo, conteggio]
    [required] { $syntax } [obbligatorio, predefinito: { $default_value }, valore del flag: { $flag_value }]
    [required_count] { $syntax } [obbligatorio, predefinito: { $default_value }, valore del flag: { $flag_value }, conteggio]
    [required_multiple] { $syntax } [obbligatorio, predefinito: { $default_value }, valore del flag: { $flag_value }, multiplo]
    [required_multiple_count] { $syntax } [obbligatorio, predefinito: { $default_value }, valore del flag: { $flag_value }, multiplo, conteggio]
}

journal.cli.sol.help.parameter.flag = { $shape ->
    *[plain] { $syntax } [valore del flag: { $flag_value }]
    [count] { $syntax } [valore del flag: { $flag_value }, conteggio]
    [multiple] { $syntax } [valore del flag: { $flag_value }, multiplo]
    [multiple_count] { $syntax } [valore del flag: { $flag_value }, multiplo, conteggio]
    [required] { $syntax } [obbligatorio, valore del flag: { $flag_value }]
    [required_count] { $syntax } [obbligatorio, valore del flag: { $flag_value }, conteggio]
    [required_multiple] { $syntax } [obbligatorio, valore del flag: { $flag_value }, multiplo]
    [required_multiple_count] { $syntax } [obbligatorio, valore del flag: { $flag_value }, multiplo, conteggio]
}

journal.cli.sol.help.section.arguments = Argomenti:

journal.cli.sol.help.section.commands = Comandi:

journal.cli.sol.help.section.options = Opzioni:

journal.cli.sol.help.title = { $solstone_name } - il tuo diario, dalla riga di comando

journal.cli.sol.help.tools = Strumenti

journal.cli.sol.import.already.started = già avviato{ $line_break }

journal.cli.sol.import.argument.media = i seguenti argomenti sono obbligatori: media

journal.cli.sol.import.argument.unexpected = argomento/i imprevisto/i: { $arguments }

journal.cli.sol.import.argument.unknown = argomenti non riconosciuti: { $token }

journal.cli.sol.import.duplicate.entries = { $entries_quantity ->
    [one] { $solstone_import }: già importato il { $date } ({ $entries } elemento); viene saltato
    [many] { $solstone_import }: già importato il { $date } ({ $entries } di elementi); viene saltato
   *[other] { $solstone_import }: già importato il { $date } ({ $entries } elementi); viene saltato
}

journal.cli.sol.import.duplicate.entries.literal = { $solstone_import }: già importato il { $date } ({ $entries } elementi); viene saltato{ $line_break }

journal.cli.sol.import.duplicate.entries.literal.unknown = { $solstone_import }: già importato in data sconosciuta ({ $entries } elementi); viene saltato{ $line_break }

journal.cli.sol.import.duplicate.entries.unknown = { $entries_quantity ->
    [one] { $solstone_import }: già importato in data sconosciuta ({ $entries } elemento); viene saltato
    [many] { $solstone_import }: già importato in data sconosciuta ({ $entries } di elementi); viene saltato
   *[other] { $solstone_import }: già importato in data sconosciuta ({ $entries } elementi); viene saltato
}

journal.cli.sol.import.duplicate.imported = { $solstone_import }: già importato il { $date }; viene saltato{ $line_break }

journal.cli.sol.import.duplicate.imported.unknown = { $solstone_import }: già importato in data sconosciuta; viene saltato{ $line_break }

journal.cli.sol.import.duplicate.skipping = { $solstone_import }: importazione duplicata; viene saltata{ $line_break }

journal.cli.sol.import.duplicate.staged = { $solstone_import }: già preparato come { $import_id }; viene saltato{ $line_break }

journal.cli.sol.import.duplicate.staged.unknown = { $solstone_import }: già preparato come sconosciuto; viene saltato{ $line_break }

journal.cli.sol.import.help = uso: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Importa i contenuti multimediali tramite il diario{ $line_break }

journal.cli.sol.import.host.guidance = `{ $auto } { $guidance }` richiede l'host del diario. Usa `{ $timestamp }` qui o esegui `{ $solstone_journal_importer }`.

journal.cli.sol.import.host.option = `{ $option }` richiede l'host del diario. Eseguilo sull'host del diario con `{ $solstone_journal_importer }`.

journal.cli.sol.import.journal.unreachable = { $solstone_import }: non è stato possibile connettersi al diario. Avvialo con '{ $solstone_journal_up }' e riprova.{ $line_break }

journal.cli.sol.import.notice.failed = non è stato possibile mostrare l'avviso del giorno di importazione: { $error }

journal.cli.sol.import.queued = elaborazione in coda{ $line_break }

journal.cli.sol.import.response.malformed = { $solstone_import }: non è stato possibile leggere la risposta del diario{ $line_break }

journal.cli.sol.import.size.failed = non è stato possibile mostrare la dimensione dell'importazione: { $error }

journal.cli.sol.import.size.skipped = { $skipped } saltati{ $line_break }

journal.cli.sol.import.size.total = { $recordings_quantity ->
    [one] { $recordings } registrazione in { $days_quantity ->
        [one] { $days } giorno
        [many] { $days } di giorni
       *[other] { $days } giorni
    }
    [many] { $recordings } di registrazioni in { $days_quantity ->
        [one] { $days } giorno
        [many] { $days } di giorni
       *[other] { $days } giorni
    }
   *[other] { $recordings } registrazioni in { $days_quantity ->
        [one] { $days } giorno
        [many] { $days } di giorni
       *[other] { $days } giorni
    }
}

journal.cli.sol.import.stage.failed = { $solstone_import }: non è stato possibile preparare l'importazione: { $error }{ $line_break }

journal.cli.sol.import.staged.failed = { $solstone_import }: preparato { $staged_path }, ma l'elaborazione non è stata messa in coda: { $error }{ $line_break }

journal.cli.sol.import.staged.malformed = { $solstone_import }: preparato { $staged_path }, ma l'elaborazione non è stata messa in coda: non è stato possibile leggere la risposta del diario{ $line_break }

journal.cli.sol.import.staged.path = preparato { $path }{ $line_break }

journal.cli.sol.import.staged.timestamp = data e ora { $timestamp }{ $line_break }

journal.cli.sol.import.staged.unreachable = { $solstone_import }: preparato { $staged_path }, ma l'elaborazione non è stata messa in coda: non è stato possibile connettersi al diario{ $line_break }

journal.cli.sol.import.task.queued = compito di elaborazione in coda { $task_id }{ $line_break }

journal.cli.sol.import.task.started = già avviato come compito { $task_id }{ $line_break }

journal.cli.sol.import.usage.error = uso: { $solstone_import_h_timestamp_ti }{ $line_break }{ $line_break }Importa i contenuti multimediali tramite il diario{ $line_break }{ $solstone_import }: errore: { $error }{ $line_break }

journal.cli.sol.journal.facet.delete.yes = Errore: { $yes } è obbligatorio per eliminare una facetta.

journal.cli.sol.launcher.cycle.mac = { $solstone_name }: rilevato un ciclo di collegamenti simbolici del programma di avvio nativo. Reinstalla l'app del diario.

journal.cli.sol.launcher.cycle.unix = { $solstone_name }: rilevato un ciclo di collegamenti simbolici del programma di avvio nativo. Reinstalla { $solstone_name } e { $sibling }.

journal.cli.sol.launcher.dangling.mac = { $solstone_name }: il collegamento simbolico del programma di avvio nativo è pendente: { $path }. Reinstalla l'app del diario.

journal.cli.sol.launcher.dangling.unix = { $solstone_name }: il collegamento simbolico del programma di avvio nativo è pendente: { $path }. Reinstalla { $solstone_name } e { $sibling }.

journal.cli.sol.launcher.executable.mac = { $solstone_name }: il fratello nativo { $sibling } non è eseguibile: { $path }. Reinstalla l'app del diario.

journal.cli.sol.launcher.executable.unix = { $solstone_name }: il fratello nativo { $sibling } non è eseguibile: { $path }. Reinstalla { $solstone_name } e { $sibling }.

journal.cli.sol.launcher.missing.mac = { $solstone_name }: manca il fratello nativo { $sibling }: { $path }. Reinstalla l'app del diario.

journal.cli.sol.launcher.missing.unix = { $solstone_name }: manca il fratello nativo { $sibling }: { $path }. Reinstalla { $solstone_name } e { $sibling }.

journal.cli.sol.link.argument.expected = argomento { $option }: atteso un argomento

journal.cli.sol.link.argument.unrecognized = argomenti non riconosciuti: { $unknown }

journal.cli.sol.link.bundle.chain.empty = { $chain_pem } non contiene certificati

journal.cli.sol.link.bundle.chain.incomplete = { $chain_pem } contiene un certificato incompleto

journal.cli.sol.link.bundle.endpoint.ip = all'endpoint { $lan } manca l'ip

journal.cli.sol.link.bundle.endpoint.port = l'endpoint { $lan } ha una porta non valida

journal.cli.sol.link.bundle.endpoints.list = { $peer_json } { $local_endpoints } deve essere un elenco

journal.cli.sol.link.bundle.endpoints.objects = gli elementi di { $peer_json } { $local_endpoints } devono essere oggetti

journal.cli.sol.link.bundle.exists = Il percorso delle credenziali esiste già: { $path }. Rimuovilo ed esegui di nuovo se accoppi di nuovo.

journal.cli.sol.link.bundle.exists.spent = Il percorso delle credenziali esiste già: { $path }. Il codice di accoppiamento ora è esaurito; generane uno nuovo ed esegui di nuovo dopo averlo rimosso.

journal.cli.sol.link.bundle.files.incomplete = l'insieme dei file del pacchetto di credenziali è incompleto

journal.cli.sol.link.bundle.files.missing = manca il file del pacchetto { $pl }: { $paths }

journal.cli.sol.link.bundle.home.missing = non è stato possibile risolvere la directory home per le credenziali di accoppiamento di questo dispositivo.

journal.cli.sol.link.bundle.invalid = pacchetto di link non valido per l'etichetta '{ $label }' in { $path }: { $error }. Esegui `{ $solstone_link_join }` per accoppiare questo dispositivo.

journal.cli.sol.link.bundle.multiple = trovati più pacchetti di link: { $labels }. passa { $label } per sceglierne uno.

journal.cli.sol.link.bundle.none = nessun pacchetto di link trovato sotto { $path }. esegui `{ $solstone_link_join }` per accoppiare questo dispositivo.

journal.cli.sol.link.bundle.path.missing = pacchetto { $pl } non trovato: { $path }

journal.cli.sol.link.bundle.peer.invalid = non valido { $peer_json } in { $path }: { $error }

journal.cli.sol.link.confirm.done = la marca del tuo diario è già confermata. non c'è niente da fare.{ $line_break }

journal.cli.sol.link.confirm.error = { $solstone_link_confirm }: errore: { $error }{ $line_break }

journal.cli.sol.link.confirm.help = uso: { $solstone_link_confirm_h_label_ }{ $line_break }{ $line_break }Conferma la marca del diario per un link che la attende.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $h_help }{ $space_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $label_label }{ $space_2 }Etichetta del pacchetto di link (per impostazione predefinita l'unico link accoppiato){ $line_break }{ $mark_mark }{ $space_4 }le due parole della marca che mostra l'app di rete del tuo diario.{ $line_break }{ $space_17 }serve quando non c'è un terminale su cui chiederti.{ $line_break }

journal.cli.sol.link.confirm.success = accoppiato.{ $line_break }

journal.cli.sol.link.confirm.usage.error = uso: { $solstone_link_confirm_h_label_ }{ $line_break }{ $solstone_link_confirm }: errore: { $error }{ $line_break }

journal.cli.sol.link.held = in attesa che tu confermi la marca del tuo diario. niente di ciò che è in attesa entra nel tuo diario finché non lo fai.{ $line_break }quando sei pronto, esegui: { $solstone_link_confirm } { $label_1 } { $label }{ $line_break }

journal.cli.sol.link.join.code.invalid = Il codice di accoppiamento non corrispondeva a una forma accettata. Usa un link di accoppiamento come { $https_go_solstone_app_p }... da '{ $solstone_call_link_pair }'.

journal.cli.sol.link.join.code.malformed = Link di accoppiamento malformato. Usa il valore completo { $https_go_solstone_app_p }... dall'output di accoppiamento.

journal.cli.sol.link.join.code.nonlocal = Il link di accoppiamento punta a un indirizzo che questo dispositivo in unione comporrà, fuori dalla rete locale.

journal.cli.sol.link.join.help = uso: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $h_help }{ $space_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $home_home }{ $space_4 }{ $url } di base del ricevitore{ $line_break }{ $code_code }{ $space_4 }{ $url } del link di accoppiamento{ $line_break }{ $as_as_role }{ $space_3 }Tag facoltativo con cui unirsi{ $line_break }{ $label_label }{ $space_2 }Etichetta delle credenziali locali (per impostazione predefinita il nome host di questa macchina){ $line_break }{ $mark_mark }{ $space_4 }le due parole della marca che mostra l'app di rete del tuo diario.{ $line_break }{ $space_17 }serve quando non c'è un terminale su cui chiederti.{ $line_break }

journal.cli.sol.link.join.invalid.role = ruolo non valido; atteso uno tra: { $phone }, { $observer }{ $line_break }

journal.cli.sol.link.join.lock.error = { $solstone_link_join }: errore: { $err }{ $line_break }

journal.cli.sol.link.join.paired = accoppiato.{ $line_break }credenziali: { $path }{ $line_break }

journal.cli.sol.link.join.pairing.unavailable = La giunzione di accoppiamento del link non è disponibile.{ $line_break }

journal.cli.sol.link.join.required.code = i seguenti argomenti sono obbligatori: { $code }

journal.cli.sol.link.join.response.large = Risposta di accoppiamento: { $local_endpoints } è troppo grande.{ $line_break }

journal.cli.sol.link.join.target.host = Alla destinazione del link di accoppiamento manca l'host.

journal.cli.sol.link.join.target.port = Alla destinazione del link di accoppiamento manca la porta esplicita.

journal.cli.sol.link.join.usage.error = uso: { $solstone_link_join_h_home_home }{ $line_break }{ $label_label_mark_mark }{ $line_break }{ $solstone_link_join }: errore: { $error }{ $line_break }

journal.cli.sol.link.label.character = { $label } può contenere solo lettere, numeri, '-', '_' e '.'

journal.cli.sol.link.label.empty = { $label } non deve essere vuoto

journal.cli.sol.link.label.length = { $label } deve essere di 80 caratteri o meno

journal.cli.sol.link.label.separator = { $label } non deve contenere separatori di percorso

journal.cli.sol.link.label.start = { $label } non deve iniziare con '.'

journal.cli.sol.link.label.traversal = { $label } non deve contenere '..'

journal.cli.sol.link.mark.ask.identified = corrisponde al tuo diario? scrivi { $yes } o { $no }:

journal.cli.sol.link.mark.ask.unavailable = scrivi { $continue } per accoppiare comunque, o { $cancel } per interrompere:

journal.cli.sol.link.mark.confirm.cancel = accoppiamento annullato. ottieni un nuovo link di accoppiamento dal tuo diario ed esegui di nuovo { $solstone_link_join } quando sei pronto.{ $line_break }

journal.cli.sol.link.mark.confirm.mismatch = non accoppiato.{ $line_break }hai detto che questa marca non corrisponde a quella che mostra il tuo diario, quindi questo computer non è accoppiato. potresti aver incollato il link sbagliato, o qualcosa non va. ottieni un nuovo link di accoppiamento dal tuo diario ed esegui di nuovo { $solstone_link_join }, o scrivi a { $support_solstone_app } e ti aiuteremo.{ $line_break }

journal.cli.sol.link.mark.confirm.terminal = { $solstone_link_confirm } non può chiederti della marca del tuo diario qui. eseguilo in un terminale, o indica le due parole della marca con { $mark_word_word }.{ $line_break }

journal.cli.sol.link.mark.confirm.unavailable = non è stato possibile ricavare la marca del diario su questo computer, quindi le parole che hai dato non si possono mettere in corrispondenza. in attesa che tu confermi la marca del tuo diario. niente di ciò che è in attesa entra nel tuo diario finché non lo fai. esegui { $solstone_link_confirm } in un terminale per decidere da te.{ $line_break }

journal.cli.sol.link.mark.confirm.wrong = non accoppiato.{ $line_break }le parole della marca che hai dato non corrispondono alla marca del diario, quindi questo computer non è accoppiato. controlla le parole, ottieni un nuovo link di accoppiamento dal tuo diario ed esegui di nuovo { $solstone_link_join }.{ $line_break }

journal.cli.sol.link.mark.join.cancel = accoppiamento annullato. niente di ciò che questo computer ha tenuto è andato a quel diario attraverso questo link. ottieni un nuovo link di accoppiamento dal tuo diario ed esegui di nuovo { $solstone_link_join } quando sei pronto.{ $line_break }

journal.cli.sol.link.mark.join.mismatch = non accoppiato.{ $line_break }hai detto che questa marca non corrisponde a quella che mostra il tuo diario, quindi questo computer non è accoppiato, e niente di ciò che ha tenuto è andato a quel diario attraverso questo link. potresti aver incollato il link sbagliato, o qualcosa non va. ottieni un nuovo link di accoppiamento dal tuo diario ed esegui di nuovo { $solstone_link_join }, o scrivi a { $support_solstone_app } e ti aiuteremo.{ $line_break }

journal.cli.sol.link.mark.join.unverifiable = non è stato possibile verificare.{ $line_break }non è stato possibile ricavare la marca del diario su questo computer, quindi le parole che hai dato non si possono mettere in corrispondenza. questo computer non è accoppiato, e niente di ciò che ha tenuto è andato a quel diario attraverso questo link. ottieni un nuovo link di accoppiamento dal tuo diario ed esegui { $solstone_link_join } in un terminale per decidere da te.{ $line_break }

journal.cli.sol.link.mark.join.wrong = non accoppiato.{ $line_break }le parole della marca che hai dato non corrispondono alla marca del diario, quindi questo computer non è accoppiato, e niente di ciò che ha tenuto è andato a quel diario attraverso questo link. controlla le parole, ottieni un nuovo link di accoppiamento dal tuo diario ed esegui di nuovo { $solstone_link_join }.{ $line_break }

journal.cli.sol.link.mark.prompt.identified = ancora un passo: controlla la marca del tuo diario.{ $line_break }{ $line_break }{ $space_2 }la marca del tuo diario: { $spoken }{ $line_break }{ $line_break }il tuo diario mostra questa stessa marca nella propria app di rete. dovrebbe corrispondere, esattamente.{ $line_break }corrisponde al tuo diario? scrivi { $yes } o { $no }:{ $line_break }

journal.cli.sol.link.mark.prompt.unavailable = ancora un passo: controlla la marca del tuo diario.{ $line_break }{ $line_break }{ $space_2 }la marca del tuo diario: non disponibile in questo momento{ $line_break }{ $line_break }non è stato possibile verificare.{ $line_break }non è stato possibile ricavare la marca del tuo diario su questo computer, quindi non c'è niente da confrontare. { $continue } solo se hai la certezza che il link viene dal tuo diario.{ $line_break }scrivi { $continue } per accoppiare comunque, o { $cancel } per interrompere:{ $line_break }

journal.cli.sol.link.mark.terminal.required = { $solstone_link_join } non può chiederti della marca del tuo diario qui. eseguilo in un terminale, o indica le due parole della marca con { $mark_word_word }.{ $line_break }

journal.cli.sol.link.mark.usage = dai le due parole della marca del tuo diario, come { $mark_bramble_quokka }.{ $line_break }

journal.cli.sol.link.pair.closed = Accoppiamento non riuscito: la finestra di accoppiamento è chiusa o scaduta. Genera un nuovo codice di accoppiamento e riprova.

journal.cli.sol.link.pair.enrollment = Il relay ha rifiutato l'iscrizione del dispositivo ({ $endpoint } { $http } { $status }).

journal.cli.sol.link.pair.failed = Accoppiamento non riuscito ({ $code }). Genera un nuovo codice di accoppiamento e riprova.

journal.cli.sol.link.pair.listener = Non è stato possibile connettersi al listener di accoppiamento.

journal.cli.sol.link.pair.rejected = Accoppiamento non riuscito ({ $http } { $status }): la finestra di accoppiamento è chiusa o il codice è già stato usato.

journal.cli.sol.link.relay.abnormal = La connessione del relay si è chiusa in modo anomalo. Riprova o usa { $direct } su { $lan_vpn }.

journal.cli.sol.link.relay.closed = La finestra di accoppiamento del relay è chiusa. Esegui di nuovo { $solstone_link_join } da un codice nuovo.

journal.cli.sol.link.relay.offline = il relay segnala che il tuo diario non è in linea. avvia il diario o usa { $direct } su lan/vpn.

journal.cli.sol.link.relay.overflow = Il relay è temporaneamente sovraccarico. Riprova o usa { $direct } su { $lan_vpn }.

journal.cli.sol.link.relay.stalled = La connessione del relay è in stallo. Riprova o usa { $direct } su { $lan_vpn }.

journal.cli.sol.link.relay.unauthorized = il relay ha rifiutato il token di questo dispositivo. esegui di nuovo { $solstone_link_join } per questo dispositivo.

journal.cli.sol.link.relay.unknown = il relay non conosce questo diario. esegui di nuovo { $solstone_link_join }.

journal.cli.sol.link.relay.unpaid = L'account del relay non è disponibile. Controlla lo stato del servizio/account del relay o usa { $direct }.

journal.cli.sol.link.relay.upgrade = Il relay ha rifiutato l'upgrade WebSocket. Controlla { $relay_url } e riprova.

journal.cli.sol.link.response.attestation = Nella risposta di accoppiamento manca { $home_attestation }

journal.cli.sol.link.response.cert = Nella risposta di accoppiamento manca { $client_cert }

journal.cli.sol.link.response.chain = Nella risposta di accoppiamento manca { $ca_chain }

journal.cli.sol.link.response.endpoints = Risposta di accoppiamento: { $local_endpoints } deve essere un array.

journal.cli.sol.link.response.instance = Nella risposta di accoppiamento manca { $instance_id }

journal.cli.sol.link.response.key = Nella risposta di accoppiamento manca la chiave del client generata

journal.cli.sol.link.serve.bind.failed = non è possibile riservare { $value_127_0_0_1 }{ $port }: riserva non riuscita

journal.cli.sol.link.serve.bind.in.use = non è possibile riservare { $value_127_0_0_1 }{ $port }: indirizzo già in uso. Un altro `{ $solstone_link_serve }` o Convey potrebbe già usare quella porta.

journal.cli.sol.link.serve.bridge.failed = La configurazione del ponte link nativo non è riuscita prima di servire. Riprova dopo aver reinstallato { $solstone_core }.

journal.cli.sol.link.serve.credentials.invalid = Le credenziali del link non sono valide. Esegui { $solstone_link_join } prima di { $solstone_link_serve }.

journal.cli.sol.link.serve.forward.direct = inoltro di { $value_127_0_0_1 }{ $port } -> { $mark } tramite connessione diretta{ $line_break }

journal.cli.sol.link.serve.forward.direct.unnamed = inoltro di { $value_127_0_0_1 }{ $port } -> il tuo diario tramite connessione diretta{ $line_break }

journal.cli.sol.link.serve.forward.either = inoltro di { $value_127_0_0_1 }{ $port } -> { $mark } tramite diretto o relay{ $line_break }

journal.cli.sol.link.serve.forward.either.unnamed = inoltro di { $value_127_0_0_1 }{ $port } -> il tuo diario tramite diretto o relay{ $line_break }

journal.cli.sol.link.serve.forward.relay = inoltro di { $value_127_0_0_1 }{ $port } -> { $mark } solo tramite relay{ $line_break }

journal.cli.sol.link.serve.forward.relay.unnamed = inoltro di { $value_127_0_0_1 }{ $port } -> il tuo diario solo tramite relay{ $line_break }

journal.cli.sol.link.serve.help = uso: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $h_help }{ $space_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $label_label }{ $space_9 }Etichetta del bundle di link{ $line_break }{ $port_port }{ $space_11 }Porta di loopback su cui servire (predefinito: 5015){ $line_break }{ $relay_url_relay_url }{ $line_break }{ $space_24 }Sostituisci l'{ $url } del relay{ $line_break }{ $direct }{ $space_14 }Solo diretto: connettiti al diario tramite una{ $line_break }{ $space_24 }connessione diretta, mai il relay. usa quando al tuo diario{ $line_break }{ $space_24 }ci si può connettere direttamente (stessa rete/vpn) per evitare la{ $line_break }{ $space_24 }dipendenza dal relay.{ $line_break }{ $relay_only }{ $space_10 }Solo relay: connettiti al diario tramite il relay,{ $line_break }{ $space_24 }mai una connessione diretta. usa quando le connessioni dirette{ $line_break }{ $space_24 }non devono essere tentate, anche se al tuo diario ci si può{ $line_break }{ $space_24 }connettere in locale.{ $line_break }

journal.cli.sol.link.serve.mark.unnamed = questo bundle di link non ha un id del diario, quindi non si può nominare. esegui di nuovo { $solstone_link_join }.{ $line_break }

journal.cli.sol.link.serve.policy.conflict = argomento { $direct }: non consentito con l'argomento { $relay_only }

journal.cli.sol.link.serve.port.invalid = argomento { $port }: valore int non valido: '{ $value }'

journal.cli.sol.link.serve.port.range = { $port } deve essere compreso tra 1 e 65535

journal.cli.sol.link.serve.runtime.unavailable = L'ambiente di esecuzione nativo del link non è disponibile. Reinstalla { $solstone_core } e riprova.

journal.cli.sol.link.serve.unavailable = Il seam di link serve non è disponibile.{ $line_break }

journal.cli.sol.link.serve.usage.error = uso: { $solstone_link_serve_h_label_la }{ $line_break }{ $relay_url_relay_url_direct_rel }{ $line_break }{ $solstone_link_serve }: errore: { $error }{ $line_break }

journal.cli.sol.link.serve.warning = avviso: uso di sviluppo solo su sistemi attendibili.{ $line_break }questo ponte dà ai chiamanti locali la piena autorità { $api } del diario del titolare accoppiato.{ $line_break }eseguilo solo dove ti fidi di tutto il software in esecuzione su questo sistema.

journal.cli.sol.link.sighting.certainty = { $line_break }{ $line_break }questo è meno certo di una connessione diretta.

journal.cli.sol.link.sighting.claim.unshown = cosa ha risposto:{ $space_2 }non è stato possibile mostrare l'identità dichiarata (dichiarata, non verificata)

journal.cli.sol.link.sighting.claimed = cosa ha risposto:{ $space_2 }{ $spoken } (dichiarato, non verificato)

journal.cli.sol.link.sighting.direct = diario sconosciuto visto a { $addr }:

journal.cli.sol.link.sighting.journal = il tuo diario:{ $space_3 }{ $spoken }

journal.cli.sol.link.sighting.no.identity = cosa ha risposto:{ $space_2 }nessuna identità presentata

journal.cli.sol.link.sighting.relay = diario sconosciuto visto tramite il relay:

journal.cli.sol.link.sighting.reset = { $line_break }{ $line_break }se hai reimpostato di recente il tuo diario, accoppia di nuovo questo dispositivo.

journal.cli.sol.link.status.error = { $solstone_link_status }: errore: { $error }{ $line_break }

journal.cli.sol.link.status.help = uso: { $solstone_link_status_h_label_l }{ $line_break }{ $line_break }Mostra lo stato del link e la versione osservata del diario remoto.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $h_help }{ $space_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $label_label }{ $space_2 }Etichetta del bundle di link (il predefinito è l'unico link accoppiato){ $line_break }

journal.cli.sol.link.status.label = Etichetta: { $label }{ $line_break }

journal.cli.sol.link.status.live = Stato: { $state }{ $line_break }

journal.cli.sol.link.status.persist.warning = Avviso: le modifiche della connessione potrebbero non essere state salvate. conserva le tue credenziali; lo stato precedente potrebbe essere ancora su disco.{ $line_break }

journal.cli.sol.link.status.stopped = Stato: fermato{ $line_break }

journal.cli.sol.link.status.usage.error = uso: { $solstone_link_status_h_label_l }{ $line_break }{ $solstone_link_status }: errore: { $error }{ $line_break }

journal.cli.sol.link.status.version.fresh = Versione del diario: { $version }{ $line_break }

journal.cli.sol.link.status.version.last = Versione del diario: { $version } (ultima conosciuta){ $line_break }

journal.cli.sol.link.status.version.unknown = Versione del diario: sconosciuta{ $line_break }

journal.cli.sol.link.transport.credential = il materiale della credenziale del link non è valido. esegui di nuovo { $solstone_link_join } per questo dispositivo.

journal.cli.sol.link.transport.enroll.rejected = L'iscrizione al relay è stata rifiutata con { $http } { $status }. Esegui di nuovo { $solstone_link_join } se l'attestazione del bundle non è aggiornata.

journal.cli.sol.link.transport.enrollment = La credenziale del link o l'iscrizione al relay non è riuscita. Esegui di nuovo { $solstone_link_join } se riprovare non lo risolve.

journal.cli.sol.link.transport.framing = il framing del flusso { $spl } non è riuscito durante il servizio. Riprova; accoppia di nuovo se continua.

journal.cli.sol.link.transport.http = Non è stato possibile analizzare la risposta del diario sul link. Aggiorna entrambi i peer o riprova.

journal.cli.sol.link.transport.io = L'I/O di trasporto del link non è riuscito durante il servizio. Controlla che al diario ci si possa connettere su { $lan_vpn } o sul relay, poi riprova.

journal.cli.sol.link.transport.json = Non è stato possibile analizzare il { $json } del relay o del ponte. Controlla l'{ $url } del relay e riprova.

journal.cli.sol.link.transport.no.endpoint = Nessun endpoint del diario è disponibile. Esegui di nuovo { $solstone_link_join } o passa { $relay_url } salvo uso intenzionale di { $direct }.

journal.cli.sol.link.transport.not.paired = Mancano le credenziali del link. Esegui { $solstone_link_join } prima di { $solstone_link_serve }.

journal.cli.sol.link.transport.offset = La ricerca dell'offset locale non è riuscita. Controlla l'orologio di sistema e riprova.

journal.cli.sol.link.transport.pair.data = i dati di accoppiamento memorizzati non sono validi. esegui di nuovo { $solstone_link_join } per questo dispositivo.

journal.cli.sol.link.transport.refresh.rejected = l'aggiornamento del token del relay è stato rifiutato con { $http } { $status }. esegui di nuovo { $solstone_link_join } per questo dispositivo.

journal.cli.sol.link.transport.rejected = Il diario accoppiato ha rifiutato la richiesta di link con { $http } { $status }.

journal.cli.sol.link.transport.tls = L'handshake sicuro del link non è riuscito. Esegui di nuovo { $solstone_link_join } se il certificato del diario o l'accoppiamento sono cambiati.

journal.cli.sol.link.unpaired.instructions = Questo dispositivo non può connettersi al tuo diario con l'accoppiamento salvato. Per accoppiarlo di nuovo, ferma '{ $solstone_link_serve }', rimuovi la cartella { $path }, esegui '{ $solstone_link_join_code_new_pa }', poi avvia di nuovo '{ $solstone_link_serve }'.{ $line_break }

journal.cli.sol.moved = '{ $command }' è stato spostato in '{ $journal_command } { $command }'; esegui quello invece.{ $line_break }(i comandi che eseguono il diario su questo computer sono sotto '{ $journal_command }'; vedi '{ $journal_help }'.)

journal.cli.sol.moved.navigate = Spostato in `{ $solstone_journal_navigate }` — esegui quello invece.{ $line_break }

journal.cli.sol.profile.cadence = Cadenza:

journal.cli.sol.profile.description = descrizione: { $value }

journal.cli.sol.profile.detached = facette separate: { $detached_label }

journal.cli.sol.profile.entity.id = entity_id: { $value }

journal.cli.sol.profile.full.interactions = { $space_2 }recent_interactions_count_30d: { $value }

journal.cli.sol.profile.full.interval = { $space_2 }avg_interval_days: { $value }

journal.cli.sol.profile.full.last.seen = { $space_2 }last_seen: { $value }

journal.cli.sol.profile.full.quiet = { $space_2 }gone_quiet_since: { $value }

journal.cli.sol.profile.identity = { $name } · { $kind } · facets={ $facets } · self={ $self_value } · hidden={ $hidden }

journal.cli.sol.profile.interactions = recent_interactions_count_30d: { $value }

journal.cli.sol.profile.interval = avg_interval_days: { $value }

journal.cli.sol.profile.last.seen = last_seen: { $value }

journal.cli.sol.profile.name = nome: { $value }

journal.cli.sol.profile.not.found = profilo non trovato: { $name }

journal.cli.sol.profile.quiet = gone_quiet_since: { $value }

journal.cli.sol.profile.type = tipo: { $value }

journal.cli.sol.resident.platform = i segnali di arresto non sono disponibili su questa piattaforma

journal.cli.sol.resident.signal = configurazione del segnale del residente nativo { $solstone_name } non riuscita: { $error }

journal.cli.sol.resident.startup = flush di avvio del residente nativo { $solstone_name } non riuscito: { $error }

journal.cli.sol.root.inspect = risoluzione della radice del progetto nativo { $solstone_name } non riuscita: non è stato possibile ispezionare l'eseguibile corrente: { $error }

journal.cli.sol.root.package = risoluzione della radice del progetto nativo { $solstone_name } non riuscita: non è stato possibile individuare il checkout dei sorgenti o il pacchetto { $solstone_name } installato da { $path }

journal.cli.sol.skills.agent.invalid = { $skill_command }: errore: { $agent_flag } { $agent } non supportato

journal.cli.sol.skills.agent.required = { $skill_command }: errore: { $agent_flag } richiede un valore

journal.cli.sol.skills.argument.invalid = { $skill_command }: errore: argomento { $argument } non riconosciuto

journal.cli.sol.skills.bundle.linked = lo skill in bundle contiene elementi collegati o speciali

journal.cli.sol.skills.cleanup.failed = aggiornamento degli skill dell'utente: installato { $path }; pulizia del vecchio skill in { $backup } non riuscita: { $error }

journal.cli.sol.skills.config.decode = errore di decodifica del diario di config: { $kind }

journal.cli.sol.skills.config.not.directory = { $path } non è una directory

journal.cli.sol.skills.copy.special = i file speciali non sono consentiti negli alberi degli skill dell'utente

journal.cli.sol.skills.copy.symlink = i symlink non sono consentiti negli alberi degli skill dell'utente

journal.cli.sol.skills.cwd.failed = { $solstone }: la cwd nativa degli skill non è disponibile: { $error }. Esegui questo comando da una directory accessibile.

journal.cli.sol.skills.exe.current = non è stato possibile determinare l'exe corrente: { $error }

journal.cli.sol.skills.exe.parent = l'exe corrente non ha una directory superiore

journal.cli.sol.skills.file.utf = { $skill_file } non è un { $encoding } valido: { $error }

journal.cli.sol.skills.home.absolute = home deve essere un percorso assoluto

journal.cli.sol.skills.home.missing = { $solstone }: la home nativa degli skill non è disponibile. Imposta { $home } sulla tua directory home.

journal.cli.sol.skills.identity.census = lettura del censimento non riuscita: { $detail }

journal.cli.sol.skills.identity.home.must.be.absolute = home deve essere assoluta

journal.cli.sol.skills.identity.installation.identity.is.unsupported.for.android = l'identità di installazione non è supportata per Android

journal.cli.sol.skills.identity.io.clone.namespace.directory = clonazione della directory dello spazio dei nomi: { $error }

journal.cli.sol.skills.identity.io.clone.namespace.registry = clonazione del registry dello spazio dei nomi: { $error }

journal.cli.sol.skills.identity.io.open.home.directory = apertura della directory home: { $error }

journal.cli.sol.skills.identity.io.open.namespace.directory = apertura della directory dello spazio dei nomi: { $error }

journal.cli.sol.skills.identity.io.open.namespace.registry = apertura del registry dello spazio dei nomi: { $error }

journal.cli.sol.skills.identity.io.read.namespace.registry = lettura del registry dello spazio dei nomi: { $error }

journal.cli.sol.skills.identity.io.stat.storage.directory = stat della directory di storage: { $error }

journal.cli.sol.skills.identity.io.stat.storage.file = stat del file di storage: { $error }

journal.cli.sol.skills.identity.namespace.entry.is.not.lowercase.hex = l'elemento dello spazio dei nomi non è hex minuscolo

journal.cli.sol.skills.identity.namespace.registry.entries.limit = il registry dello spazio dei nomi supera 1024 elementi

journal.cli.sol.skills.identity.namespace.registry.size.limit = il registry dello spazio dei nomi supera 16 MiB

journal.cli.sol.skills.identity.path.escapes.its.absolute.root = il percorso esce dalla propria radice assoluta

journal.cli.sol.skills.identity.path.must.be.absolute.and.nul.free = il percorso deve essere assoluto e senza NUL

journal.cli.sol.skills.identity.root = non è stato possibile derivare il token radice da { $path }: { $detail }

journal.cli.sol.skills.identity.storage.directory.does.not.have.private.mode = la directory di storage non ha il modo { $mode }

journal.cli.sol.skills.identity.storage.directory.is.not.owned.by.the.current.user = la directory di storage non appartiene all'utente corrente

journal.cli.sol.skills.identity.storage.file.does.not.have.private.mode = il file di storage non ha il modo { $mode }

journal.cli.sol.skills.identity.storage.file.is.not.owned.by.the.current.user = il file di storage non appartiene all'utente corrente

journal.cli.sol.skills.install.done = installato { $agent } { $skill } -> { $path }

journal.cli.sol.skills.journal.absolute = il diario selezionato deve essere un percorso assoluto

journal.cli.sol.skills.journal.resolved = il percorso risolto del diario non è assoluto

journal.cli.sol.skills.operation.error = errore: { $operation } { $path }: { $detail }

journal.cli.sol.skills.owner.base = non è stato possibile formare la base del titolare in { $path }: { $detail }

journal.cli.sol.skills.path.absolute = il percorso deve essere assoluto

journal.cli.sol.skills.path.ancestor.absent = nessun antenato esistente trovato per { $path }

journal.cli.sol.skills.path.canonical = canonicalizzazione dell'antenato { $path } non riuscita: { $error }

journal.cli.sol.skills.path.inspect = ispezione dell'antenato { $path } non riuscita: { $error }

journal.cli.sol.skills.path.overlap = il percorso { $path } si sovrappone al diario protetto

journal.cli.sol.skills.path.resolved.overlap = il percorso risolto dell'antenato { $path } si sovrappone al diario protetto

journal.cli.sol.skills.payload.failed = { $solstone }: la radice del payload degli skill nativi non è disponibile: { $detail }. Reinstalla { $solstone } e { $core }.

journal.cli.sol.skills.payload.project = errore: atteso uno skill di progetto in { $path }

journal.cli.sol.skills.payload.umbrella = errore: atteso uno skill ombrello in bundle in { $skill_file } ({ $path })

journal.cli.sol.skills.project.agent = errore: { $agent_flag } { $agent } non è supportato con { $project_flag }; usa { $all_option } o { $claude_option }

journal.cli.sol.skills.project.missing = atteso uno skill di progetto in { $path }

journal.cli.sol.skills.publish.rollback = pubblicazione dello skill non riuscita: { $error }; ripristino non riuscito: { $restore }; percorso del backup: { $path }

journal.cli.sol.skills.registry.unknown = il registry di installazione è sconosciuto

journal.cli.sol.skills.remove.directory = rifiuto di rimuovere ciò che non è una directory

journal.cli.sol.skills.remove.done = rimosso { $agent } { $skill } -> { $path }

journal.cli.sol.skills.remove.retired = rimosso { $agent } { $skill } (skill di prodotto ritirato) -> { $path }

journal.cli.sol.skills.remove.stale = rimosso { $agent } { $skill } (non aggiornato) -> { $path }

journal.cli.sol.skills.remove.symlink = rifiuto di rimuovere ciò che non è un symlink

journal.cli.sol.skills.replace.done = sostituito { $agent } { $skill } -> { $path }

journal.cli.sol.skills.skip.absent = saltato { $agent } { $skill } (niente da rimuovere)

journal.cli.sol.skills.skip.all = nessuna directory di config di agenti di programmazione IA trovata — salto della registrazione degli skill

journal.cli.sol.skills.skip.config = saltato { $agent } (directory di config assente in { $path })

journal.cli.sol.skills.status.absent = { $agent } { $skill } non installato

journal.cli.sol.skills.status.header = agente      skill                stato

journal.cli.sol.skills.status.installed = { $agent } { $skill } installato

journal.cli.sol.skills.subcommand.invalid = { $skill_command }: errore: sottocomando sconosciuto { $command_name }

journal.cli.sol.skills.subcommand.missing = { $skill_command }: errore: sottocomando mancante

journal.cli.sol.skills.symlink.unsupported = creazione di symlink non supportata

journal.cli.sol.skills.target.ineligible = la destinazione non è idonea all'installazione

journal.cli.sol.skills.target.parent = la destinazione non ha una directory superiore

journal.cli.sol.skills.target.preserved = il percorso di destinazione si sovrappone al diario protetto o è preservato

journal.cli.sol.skills.tree.directory = l'albero degli skill richiede una directory ordinaria

journal.cli.sol.skills.tree.linked = gli elementi collegati non sono consentiti negli alberi degli skill dell'utente

journal.cli.sol.skills.tree.reference = il riferimento allo skill in bundle richiede una directory con un { $skill_file } regolare

journal.cli.sol.skills.tree.special = gli elementi speciali non sono consentiti negli alberi degli skill dell'utente

journal.cli.sol.skills.usage = Uso: { $syntax }

journal.cli.sol.skills.warning.existing = avviso { $agent } { $skill } -> { $path } (contenuto esistente alla destinazione preservato)

journal.cli.sol.skills.warning.retired = avviso { $agent } { $skill } -> { $path } (non corrisponde allo skill { $retired_skill } ritirato; contenuto esistente in { $path } preservato)

journal.cli.sol.skills.warning.stale = avviso { $agent } { $skill } -> { $path } (contenuto esistente preservato alla destinazione non aggiornata)

journal.cli.sol.skills.warnings = Avvisi:

journal.cli.sol.speakers.attribute.segment.accumulated.voiceprints = { $line_break }Impronte vocali accumulate:

journal.cli.sol.speakers.attribute.segment.by.method = { $line_break }Per metodo:

journal.cli.sol.speakers.attribute.segment.embeddings = { $quantity ->
    [one] { $spacing_2 }{ $entity_id }: { $embeddings } embedding
    [many] { $spacing_2 }{ $entity_id }: { $embeddings } di embedding
   *[other] { $spacing_2 }{ $entity_id }: { $embeddings } embedding
    }

journal.cli.sol.speakers.attribute.segment.resolved = Risolti:{ $spacing_2 }{ $resolved }

journal.cli.sol.speakers.attribute.segment.sentences = Frasi: { $count }

journal.cli.sol.speakers.attribute.segment.unmatched = Senza corrispondenza: { $count }

journal.cli.sol.speakers.attribute.segment.wrote = { $line_break }Scritto: { $path }

journal.cli.sol.speakers.backfill.completed.count = Conteggio completato:{ $spacing_11 }{ $count }

journal.cli.sol.speakers.backfill.elapsed.s = Trascorso:{ $spacing_19 }{ $elapsed }s

journal.cli.sol.speakers.backfill.error.count = Conteggio errori:{ $spacing_15 }{ $count }

journal.cli.sol.speakers.backfill.errors.and.more = { $spacing_2 }... e { $count } in più

journal.cli.sol.speakers.backfill.failure.detail = Dettaglio del fallimento:{ $spacing_12 }{ $detail }

journal.cli.sol.speakers.backfill.failure.stage = Fase del fallimento:{ $spacing_13 }{ $stage }

journal.cli.sol.speakers.backfill.last.seen.entities.seen = Entità viste:{ $spacing_12 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.pending.by.entity = { $line_break }In attesa per entità:

journal.cli.sol.speakers.backfill.last.seen.rows.pending = Righe in attesa:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.rows.written = Righe scritte:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.last.seen.speaker.label.files.read = File di etichette dei parlanti letti: { $count }

journal.cli.sol.speakers.backfill.last.seen.voiceprint.rows.scanned = Righe di impronte vocali scansionate:{ $spacing_2 }{ $count }

journal.cli.sol.speakers.backfill.note.backfill.operation.has.immutable = Nota: l'operazione di backfill { $id } ha flag immutabili.{ $line_break }Per riprendere con impostazioni compatibili, prova:{ $line_break }{ $spacing_2 }{ $command } { $option_operation_id } { $id } { $option_commit }{ $line_break }

journal.cli.sol.speakers.backfill.operation.id = ID operazione:{ $spacing_14 }{ $op_id }

journal.cli.sol.speakers.backfill.pending.count = Conteggio in attesa:{ $spacing_13 }{ $count }

journal.cli.sol.speakers.backfill.protected.skipped = Protetti (saltati):{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.selected.for.backfill = Selezionati per il backfill:{ $spacing_5 }{ $count }

journal.cli.sol.speakers.backfill.starting.speaker.attribution.backfill = Avvio del backfill di attribuzione dei parlanti in corso...

journal.cli.sol.speakers.backfill.status = Stato:{ $spacing_20 }{ $status }

journal.cli.sol.speakers.backfill.total.planned.count = Conteggio totale pianificato:{ $spacing_7 }{ $count }

journal.cli.sol.speakers.backfill.total.segments.scanned = Totale segmenti scansionati:{ $spacing_4 }{ $count }

journal.cli.sol.speakers.bootstrap.bootstrapping.voiceprints.from.single.speaker = Bootstrap delle impronte vocali dai segmenti a parlante singolo in corso...

journal.cli.sol.speakers.bootstrap.stats.embeddings.saved = Embedding salvati: { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.duplicate = Embedding saltati (duplicato): { $count }

journal.cli.sol.speakers.bootstrap.stats.embeddings.skipped.owner = Embedding saltati (titolare): { $count }

journal.cli.sol.speakers.bootstrap.stats.entities.created = Entità create: { $count }

journal.cli.sol.speakers.bootstrap.stats.segments.scanned = Segmenti scansionati: { $count }

journal.cli.sol.speakers.bootstrap.stats.single.speaker.segments = Segmenti a parlante singolo: { $count }

journal.cli.sol.speakers.bootstrap.stats.top.speakers.by.embedding.count = Parlanti principali per conteggio di embedding:

journal.cli.sol.speakers.bootstrap.stats.unique.speakers = Parlanti unici: { $count }

journal.cli.sol.speakers.build.from.tags.can.build.from.tags = Si può costruire dai tag: { $available }

journal.cli.sol.speakers.build.from.tags.guidance = Guida: { $guidance }

journal.cli.sol.speakers.build.from.tags.manual.tags = Tag manuali: { $tags }

journal.cli.sol.speakers.build.from.tags.next.step = Prossimo passo: { $step }

journal.cli.sol.speakers.build.from.tags.observed = Osservato: { $observed }

journal.cli.sol.speakers.build.from.tags.owner.centroid.already.confirmed = Centroide del titolare già confermato.

journal.cli.sol.speakers.build.from.tags.owner.centroid.confirmed.from.manual = Centroide del titolare confermato dai tag manuali (principale: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.build.from.tags.owner.manual.tags.are.not = I tag manuali del titolare non sono pronti.

journal.cli.sol.speakers.build.from.tags.reason = Motivo: { $reason }

journal.cli.sol.speakers.build.from.tags.threshold = Soglia: { $threshold }

journal.cli.sol.speakers.confirm.owner.review.error.owner.voice.candidates.must = errore: I candidati della voce del titolare devono essere confermati nel browser.{ $line_break }Rivedi e conferma a { $review_entry }

journal.cli.sol.speakers.correct.already.correct = Già corretto: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.correct.corrected = Corretto { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }: { $old } -> { $new_speaker }

journal.cli.sol.speakers.correct.preview.with.solstone.call.speakers = Anteprima con: { $command } { $old_speaker } { $new_speaker }

journal.cli.sol.speakers.correct.propagation.available.statements.in.segments = { $quantity ->
    [one] { $segments_quantity ->
        [one] Propagazione disponibile: { $statements } affermazione in { $segments } segmento cambierebbe
        [many] Propagazione disponibile: { $statements } affermazione in { $segments } di segmenti cambierebbe
       *[other] Propagazione disponibile: { $statements } affermazione in { $segments } segmenti cambierebbe
    }
    [many] { $segments_quantity ->
        [one] Propagazione disponibile: { $statements } di affermazioni in { $segments } segmento cambierebbero
        [many] Propagazione disponibile: { $statements } di affermazioni in { $segments } di segmenti cambierebbero
       *[other] Propagazione disponibile: { $statements } di affermazioni in { $segments } segmenti cambierebbero
    }
   *[other] { $segments_quantity ->
        [one] Propagazione disponibile: { $statements } affermazioni in { $segments } segmento cambierebbero
        [many] Propagazione disponibile: { $statements } affermazioni in { $segments } di segmenti cambierebbero
       *[other] Propagazione disponibile: { $statements } affermazioni in { $segments } segmenti cambierebbero
    }
    }

journal.cli.sol.speakers.correct.propagation.nothing.else.would.change = Propagazione: nient'altro cambierebbe

journal.cli.sol.speakers.correct.unassigned.unassigned = non assegnato

journal.cli.sol.speakers.correct.voiceprint.removal = Rimozione dell'impronta vocale: { $outcome }

journal.cli.sol.speakers.day.segments.showing.of.segments.limit = { $quantity ->
    [one] Visualizzazione di { $shown } di { $total } segmento (limite { $limit })
    [many] Visualizzazione di { $shown } di { $total } di segmenti (limite { $limit })
   *[other] Visualizzazione di { $shown } di { $total } segmenti (limite { $limit })
    }

journal.cli.sol.speakers.discover.cluster.samples.across.segments = { $quantity ->
    [one] { $segments_quantity ->
        [one] { $spacing_2 }Cluster { $cluster }: { $samples } campione su { $segments } segmento
        [many] { $spacing_2 }Cluster { $cluster }: { $samples } campione su { $segments } di segmenti
       *[other] { $spacing_2 }Cluster { $cluster }: { $samples } campione su { $segments } segmenti
    }
    [many] { $segments_quantity ->
        [one] { $spacing_2 }Cluster { $cluster }: { $samples } di campioni su { $segments } segmento
        [many] { $spacing_2 }Cluster { $cluster }: { $samples } di campioni su { $segments } di segmenti
       *[other] { $spacing_2 }Cluster { $cluster }: { $samples } di campioni su { $segments } segmenti
    }
   *[other] { $segments_quantity ->
        [one] { $spacing_2 }Cluster { $cluster }: { $samples } campioni su { $segments } segmento
        [many] { $spacing_2 }Cluster { $cluster }: { $samples } campioni su { $segments } di segmenti
       *[other] { $spacing_2 }Cluster { $cluster }: { $samples } campioni su { $segments } segmenti
    }
}

journal.cli.sol.speakers.discover.found.unknown.speaker.cluster.s = { $quantity ->
    [one] Trovato { $clusters } cluster di un parlante sconosciuto:{ $line_break }
    [many] Trovati { $clusters } di cluster di parlanti sconosciuti:{ $line_break }
   *[other] Trovati { $clusters } cluster di parlanti sconosciuti:{ $line_break }
}

journal.cli.sol.speakers.discover.no.recurring.unknown.speakers.found = Nessun parlante sconosciuto ricorrente trovato.

journal.cli.sol.speakers.discover.sid = { $spacing_4 }- { $day }/{ $stream }/{ $segment } sid={ $sentence_id }: { $text }

journal.cli.sol.speakers.discovery.error.try.again = riprova

journal.cli.sol.speakers.discovery.warnings.warning = Avviso: { $reason_code }: { $message }

journal.cli.sol.speakers.discovery.warnings.warning.count = Avviso: { $reason_code }: { $message } count={ $count }

journal.cli.sol.speakers.errors.errors = { $line_break }Errori ({ $count }):

journal.cli.sol.speakers.follow.backfill.backfill.in.progress.status.completed = Riempimento retroattivo in corso: status={ $status_kind }, completed={ $completed }/{ $total }, pending={ $pending }

journal.cli.sol.speakers.follow.backfill.backfill.not.completed.status.to = { $line_break }Riempimento retroattivo non completato (stato: { $status_kind }).{ $line_break }Per controllare lo stato:{ $line_break }{ $spacing_2 }{ $command_1 } { $operation_id }{ $line_break }Per riprendere:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.follow.repair.start.repair.in.progress.segments.repaired = Riparazione in corso, segmenti riparati finora: { $count }

journal.cli.sol.speakers.follow.repair.start.repair.running = Riparazione in esecuzione: { $operation_id }

journal.cli.sol.speakers.identify.error.inspect.operations.with.solstone.call = Ispeziona le operazioni con: { $command }

journal.cli.sol.speakers.identify.error.inspect.this.operation.with.solstone = Ispeziona questa operazione con: { $command } { $operation_id }

journal.cli.sol.speakers.identify.error.retry.with.the.same.request = Riprova con lo stesso { $option_request_id } { $request_id }.

journal.cli.sol.speakers.identify.name.required.usage = Uso: { $command } { $syntax }{ $line_break }Prova '{ $help_command }' per aiuto.{ $line_break }{ $box_left } Errore { $box_top }{ $line_break }{ $box_edge } Valore non valido: il nome o { $entity_option } è obbligatorio{ $box_padding }{ $box_edge }{ $line_break }{ $box_bottom }{ $line_break }

journal.cli.sol.speakers.parse.stream.layout.option.error.invalid.value.for.stream = Errore: valore non valido per { $option_stream_layout }

journal.cli.sol.speakers.presence.candidate.meeting.unvoiced.screen.meeting.days.voice.no = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.meeting.voiced.screen.meeting.days.voice.yes = { $spacing_2 }{ $name } ({ $entity }): screen={ $first_count }, meeting_days={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.candidate.mention.unvoiced.setting.speakers.voice.no = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=no

journal.cli.sol.speakers.presence.candidate.mention.voiced.setting.speakers.voice.yes = { $spacing_2 }{ $name } ({ $entity }): setting={ $first_count }, speakers={ $second_count }, voice=yes

journal.cli.sol.speakers.presence.cluster.statements.segments.conversations = { $quantity ->
    [one] { $segments_quantity ->
        [one] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } affermazione, { $segments } segmento, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } affermazione, { $segments } segmento, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } affermazione, { $segments } segmento, { $conversations } conversazioni
        }
        [many] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } affermazione, { $segments } di segmenti, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } affermazione, { $segments } di segmenti, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } affermazione, { $segments } di segmenti, { $conversations } conversazioni
        }
       *[other] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } affermazione, { $segments } segmenti, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } affermazione, { $segments } segmenti, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } affermazione, { $segments } segmenti, { $conversations } conversazioni
        }
    }
    [many] { $segments_quantity ->
        [one] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } di affermazioni, { $segments } segmento, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } di affermazioni, { $segments } segmento, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } di affermazioni, { $segments } segmento, { $conversations } conversazioni
        }
        [many] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } di affermazioni, { $segments } di segmenti, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } di affermazioni, { $segments } di segmenti, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } di affermazioni, { $segments } di segmenti, { $conversations } conversazioni
        }
       *[other] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } di affermazioni, { $segments } segmenti, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } di affermazioni, { $segments } segmenti, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } di affermazioni, { $segments } segmenti, { $conversations } conversazioni
        }
    }
   *[other] { $segments_quantity ->
        [one] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } affermazioni, { $segments } segmento, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } affermazioni, { $segments } segmento, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } affermazioni, { $segments } segmento, { $conversations } conversazioni
        }
        [many] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } affermazioni, { $segments } di segmenti, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } affermazioni, { $segments } di segmenti, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } affermazioni, { $segments } di segmenti, { $conversations } conversazioni
        }
       *[other] { $conversations_quantity ->
            [one] Cluster { $cluster }: { $statements } affermazioni, { $segments } segmenti, { $conversations } conversazione
            [many] Cluster { $cluster }: { $statements } affermazioni, { $segments } segmenti, { $conversations } di conversazioni
           *[other] Cluster { $cluster }: { $statements } affermazioni, { $segments } segmenti, { $conversations } conversazioni
        }
    }
}

journal.cli.sol.speakers.presence.cluster.was.not.found.run = Cluster { $cluster_id } non è stato trovato.{ $line_break }Esegui '{ $command }' per produrre id di cluster validi.{ $line_break }

journal.cli.sol.speakers.presence.co.presence = { $line_break }Copresenza:

journal.cli.sol.speakers.presence.evidence.complete = Prova: completa

journal.cli.sol.speakers.presence.evidence.gap.s = { $quantity ->
    [one] Prova: { $gaps } lacuna
    [many] Prova: { $gaps } di lacune
   *[other] Prova: { $gaps } lacune
}

journal.cli.sol.speakers.presence.mentions = { $line_break }Menzioni:

journal.cli.sol.speakers.presence.no.candidate.entities.found = Nessuna entità candidata trovata.

journal.cli.sol.speakers.propagate.applied.applied.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
        [one] Applicata: { $statement_count } affermazione in { $segment_count } segmento
        [many] Applicata: { $statement_count } affermazione in { $segment_count } di segmenti
       *[other] Applicata: { $statement_count } affermazione in { $segment_count } segmenti
    }
    [many] { $segment_count_quantity ->
        [one] Applicate: { $statement_count } di affermazioni in { $segment_count } segmento
        [many] Applicate: { $statement_count } di affermazioni in { $segment_count } di segmenti
       *[other] Applicate: { $statement_count } di affermazioni in { $segment_count } segmenti
    }
   *[other] { $segment_count_quantity ->
        [one] Applicate: { $statement_count } affermazioni in { $segment_count } segmento
        [many] Applicate: { $statement_count } affermazioni in { $segment_count } di segmenti
       *[other] Applicate: { $statement_count } affermazioni in { $segment_count } segmenti
    }
}

journal.cli.sol.speakers.propagate.errors = { $line_break }Errori:

journal.cli.sol.speakers.propagate.nothing.else.would.change = Nient'altro cambierebbe.

journal.cli.sol.speakers.propagate.reverse.with.solstone.call.commit = Inverti con: { $command } { $operation } { $old_speaker } { $new_speaker } { $option_commit }

journal.cli.sol.speakers.propagate.would.change.would.change.statements.in.segments = { $quantity ->
    [one] { $segment_count_quantity ->
        [one] Cambierebbe: { $statement_count } affermazione in { $segment_count } segmento
        [many] Cambierebbe: { $statement_count } affermazione in { $segment_count } di segmenti
       *[other] Cambierebbe: { $statement_count } affermazione in { $segment_count } segmenti
    }
    [many] { $segment_count_quantity ->
        [one] Cambierebbero: { $statement_count } di affermazioni in { $segment_count } segmento
        [many] Cambierebbero: { $statement_count } di affermazioni in { $segment_count } di segmenti
       *[other] Cambierebbero: { $statement_count } di affermazioni in { $segment_count } segmenti
    }
   *[other] { $segment_count_quantity ->
        [one] Cambierebbero: { $statement_count } affermazioni in { $segment_count } segmento
        [many] Cambierebbero: { $statement_count } affermazioni in { $segment_count } di segmenti
       *[other] Cambierebbero: { $statement_count } affermazioni in { $segment_count } segmenti
    }
}

journal.cli.sol.speakers.rebuild.owner.guidance = Guida: { $step }

journal.cli.sol.speakers.rebuild.owner.next.step = Prossimo passo: { $next_step }

journal.cli.sol.speakers.rebuild.owner.override.applied.true = Forzatura applicata: vero

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuild.did.not = La ricostruzione del tuo centroide non ha scritto.

journal.cli.sol.speakers.rebuild.owner.owner.centroid.rebuilt.principal.cluster = Il tuo centroide è stato ricostruito (principale: { $principal }, cluster_size: { $cluster_size })

journal.cli.sol.speakers.rebuild.owner.owner.centroid.unchanged.evidence.already = Il tuo centroide è invariato; la prova corrisponde già.

journal.cli.sol.speakers.reject.owner.error.version.is.required = Errore: { $option_version } è obbligatorio.

journal.cli.sol.speakers.repair.check.status.advice.to.check.status.solstone.call = { $line_break }Per controllare lo stato:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.dry.run.complete = Completo:{ $spacing_2 }{ $complete }

journal.cli.sol.speakers.repair.dry.run.entities.with.voiceprints.to.remove = { $line_break }Entità con impronte vocali da rimuovere: { $entities }

journal.cli.sol.speakers.repair.dry.run.gaps.detected = { $line_break }Lacune rilevate ({ $gaps }):

journal.cli.sol.speakers.repair.dry.run.mode.dry.run = Modalità:{ $spacing_6 }simulazione

journal.cli.sol.speakers.repair.dry.run.rows = { $quantity ->
    [one] { $spacing_2 }- { $id } ({ $rows } riga)
    [many] { $spacing_2 }- { $id } ({ $rows } di righe)
   *[other] { $spacing_2 }- { $id } ({ $rows } righe)
}

journal.cli.sol.speakers.repair.dry.run.segments.to.repair = { $line_break }Segmenti da riparare: { $segments }

journal.cli.sol.speakers.repair.in.progress.status.advice = { $line_break }La riparazione potrebbe essere ancora in esecuzione nel diario. Non avviarne un'altra.{ $line_break }Per controllare lo stato:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.repair.starting.speaker.contamination.repair = Avvio della riparazione della contaminazione dei parlanti in corso...

journal.cli.sol.speakers.repair.status.clean = Pulito:{ $spacing_5 }{ $clean }

journal.cli.sol.speakers.repair.status.last.failure = Ultimo fallimento: { $stage } ({ $code })

journal.cli.sol.speakers.repair.status.segments.repaired = Segmenti riparati: { $count }

journal.cli.sol.speakers.repair.status.status = Stato:{ $spacing_4 }{ $status }

journal.cli.sol.speakers.repair.stopped.advice.to.resume.solstone.call.speakers = { $line_break }Per riprendere:{ $line_break }{ $spacing_2 }{ $command } { $operation_id }{ $line_break }

journal.cli.sol.speakers.report.only = SOLO RAPPORTO — passa { $option_commit } per persistere.{ $line_break }

journal.cli.sol.speakers.resolve.names.ambiguous = { $line_break }Ambiguità ({ $count }):

journal.cli.sol.speakers.resolve.names.auto.merged = { $line_break }Uniti automaticamente ({ $count }):

journal.cli.sol.speakers.resolve.names.entities.with.voiceprints = Entità con impronte vocali: { $count }

journal.cli.sol.speakers.resolve.names.high.similarity.pairs = Coppie ad alta somiglianza: { $count }

journal.cli.sol.speakers.resolve.names.merging.name.variants.can.t = L'unione delle varianti dei nomi non si può annullare. Rivedi il rapporto senza { $option_commit }, poi, una volta che approvi, eseguilo di nuovo con { $option_commit } { $option_yes }.

journal.cli.sol.speakers.resolve.names.pairs.compared = Coppie confrontate: { $count }

journal.cli.sol.speakers.resolve.names.resolving.speaker.name.variants = Risoluzione delle varianti dei nomi dei parlanti in corso...

journal.cli.sol.speakers.seed.from.imports.seeding.voiceprints.from.import.segments = Popolamento delle impronte vocali dai segmenti di importazione in corso...

journal.cli.sol.speakers.seed.from.imports.segments.with.speakers = Segmenti con parlanti: { $count }

journal.cli.sol.speakers.seed.from.imports.speakers.by.embedding.count = Parlanti per numero di embedding:

journal.cli.sol.speakers.seed.from.imports.unmatched.speakers = { $line_break }Parlanti non corrispondenti ({ $count }):

journal.cli.sol.speakers.sentences.sentences.for = Frasi per { $day }/{ $stream }/{ $segment }/{ $source }:

journal.cli.sol.speakers.tag.owner.owner.sentence.already.tagged = La tua frase già etichettata: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.tag.owner.tagged.owner.sentence = La tua frase etichettata: { $day }/{ $stream_layout }/{ $stream }/{ $segment } #{ $sentence_id }

journal.cli.sol.speakers.wipe.couldn.t.erase.now.it = non è stato possibile cancellare ora; è in attesa nel cestino

journal.cli.sol.speakers.wipe.erase.needs.commit = { $option_erase } richiede { $option_commit }

journal.cli.sol.speakers.wipe.erased.erased = cancellato

journal.cli.sol.speakers.wipe.files.b = { $quantity ->
    [one] { $label }: { $files } file ({ $bytes } B)
    [many] { $label }: { $files } di file ({ $bytes } B)
   *[other] { $label }: { $files } file ({ $bytes } B)
}

journal.cli.sol.speakers.wipe.in.the.trash = nel cestino

journal.cli.sol.speakers.wipe.in.the.trash.erases = nel cestino; cancella { $day }

journal.cli.sol.speakers.wipe.total.files.b = { $quantity ->
    [one] totale{ $spacing_14 }: { $files } file ({ $bytes } B)
    [many] totale{ $spacing_14 }: { $files } di file ({ $bytes } B)
   *[other] totale{ $spacing_14 }: { $files } file ({ $bytes } B)
}

journal.cli.sol.unsupported = Comando nativo { $solstone_name } non supportato.

journal.cli.sol.update.automatic.choice = non è stato possibile salvare la tua scelta.

journal.cli.sol.update.automatic.home = non è stato possibile trovare la tua directory dei dati.

journal.cli.sol.update.automatic.ineligible = questa installazione non può aggiornarsi da sola. gli aggiornamenti automatici funzionano su un'installazione da { $install_url }.

journal.cli.sol.update.automatic.ineligible.generic = questa installazione non può aggiornarsi da sola.

journal.cli.sol.update.automatic.linux = { $automatic_command } funziona su linux.

journal.cli.sol.update.automatic.off = gli aggiornamenti automatici sono disattivi. per aggiornare, esegui { $update_command }.

journal.cli.sol.update.automatic.on = gli aggiornamenti automatici sono attivi. il tuo diario non si riavvia mai da solo; una nuova versione si avvia la prossima volta che lo fa.

journal.cli.sol.update.automatic.package.add = installato da un pacchetto. per ottenere gli aggiornamenti tramite il programma di aggiornamento del tuo sistema, aggiungi il repository { $brand }: { $package_url }

journal.cli.sol.update.automatic.prefix = il nome della directory di installazione non può stare in un file di servizio.

journal.cli.sol.update.automatic.run = { $result ->
    [up-to-date] aggiornamento automatico: aggiornato
    [installed] aggiornamento automatico: installata una nuova versione
    [off] aggiornamento automatico: gli aggiornamenti automatici sono disattivi
    [paused] aggiornamento automatico: fermato dopo due tentativi che non hanno funzionato
    [held] aggiornamento automatico: gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamento automatico: il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamento automatico: non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamento automatico: in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamento automatico: in attesa della prossima versione
    [busy] aggiornamento automatico: un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamento automatico: questa installazione non può aggiornarsi da sola
   *[failed] aggiornamento automatico: l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.session = non è stato possibile configurare il controllo giornaliero degli aggiornamenti. la tua sessione utente di systemd potrebbe non essere in esecuzione.

journal.cli.sol.update.automatic.status = { $choice ->
    [on] aggiornamenti automatici: attivi
    [off] aggiornamenti automatici: disattivi
    [unasked] aggiornamenti automatici: non attivati
   *[paused] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato
}

journal.cli.sol.update.automatic.status.checked = { $choice ->
    [on] aggiornamenti automatici: attivi · ultimo controllo { $date }
    [off] aggiornamenti automatici: disattivi · ultimo controllo { $date }
    [unasked] aggiornamenti automatici: non attivati · ultimo controllo { $date }
   *[paused] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date }
}

journal.cli.sol.update.automatic.status.off.result = { $result ->
    [up-to-date] aggiornamenti automatici: disattivi · aggiornato
    [installed] aggiornamenti automatici: disattivi · installata una nuova versione
    [off] aggiornamenti automatici: disattivi · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: disattivi · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: disattivi · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: disattivi · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: disattivi · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: disattivi · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: disattivi · in attesa della prossima versione
    [busy] aggiornamenti automatici: disattivi · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: disattivi · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: disattivi · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.status.off.result.checked = { $result ->
    [up-to-date] aggiornamenti automatici: disattivi · ultimo controllo { $date } · aggiornato
    [installed] aggiornamenti automatici: disattivi · ultimo controllo { $date } · installata una nuova versione
    [off] aggiornamenti automatici: disattivi · ultimo controllo { $date } · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: disattivi · ultimo controllo { $date } · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: disattivi · ultimo controllo { $date } · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: disattivi · ultimo controllo { $date } · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: disattivi · ultimo controllo { $date } · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: disattivi · ultimo controllo { $date } · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: disattivi · ultimo controllo { $date } · in attesa della prossima versione
    [busy] aggiornamenti automatici: disattivi · ultimo controllo { $date } · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: disattivi · ultimo controllo { $date } · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: disattivi · ultimo controllo { $date } · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.status.on.result = { $result ->
    [up-to-date] aggiornamenti automatici: attivi · aggiornato
    [installed] aggiornamenti automatici: attivi · installata una nuova versione
    [off] aggiornamenti automatici: attivi · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: attivi · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: attivi · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: attivi · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: attivi · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: attivi · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: attivi · in attesa della prossima versione
    [busy] aggiornamenti automatici: attivi · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: attivi · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: attivi · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.status.on.result.checked = { $result ->
    [up-to-date] aggiornamenti automatici: attivi · ultimo controllo { $date } · aggiornato
    [installed] aggiornamenti automatici: attivi · ultimo controllo { $date } · installata una nuova versione
    [off] aggiornamenti automatici: attivi · ultimo controllo { $date } · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: attivi · ultimo controllo { $date } · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: attivi · ultimo controllo { $date } · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: attivi · ultimo controllo { $date } · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: attivi · ultimo controllo { $date } · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: attivi · ultimo controllo { $date } · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: attivi · ultimo controllo { $date } · in attesa della prossima versione
    [busy] aggiornamenti automatici: attivi · ultimo controllo { $date } · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: attivi · ultimo controllo { $date } · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: attivi · ultimo controllo { $date } · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.status.paused.result = { $result ->
    [up-to-date] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · aggiornato
    [installed] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · installata una nuova versione
    [off] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · in attesa della prossima versione
    [busy] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.status.paused.result.checked = { $result ->
    [up-to-date] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · aggiornato
    [installed] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · installata una nuova versione
    [off] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · in attesa della prossima versione
    [busy] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: fermati dopo due tentativi che non hanno funzionato · ultimo controllo { $date } · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.status.unasked.result = { $result ->
    [up-to-date] aggiornamenti automatici: non attivati · aggiornato
    [installed] aggiornamenti automatici: non attivati · installata una nuova versione
    [off] aggiornamenti automatici: non attivati · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: non attivati · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: non attivati · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: non attivati · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: non attivati · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: non attivati · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: non attivati · in attesa della prossima versione
    [busy] aggiornamenti automatici: non attivati · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: non attivati · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: non attivati · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.automatic.status.unasked.result.checked = { $result ->
    [up-to-date] aggiornamenti automatici: non attivati · ultimo controllo { $date } · aggiornato
    [installed] aggiornamenti automatici: non attivati · ultimo controllo { $date } · installata una nuova versione
    [off] aggiornamenti automatici: non attivati · ultimo controllo { $date } · gli aggiornamenti automatici sono disattivi
    [paused] aggiornamenti automatici: non attivati · ultimo controllo { $date } · fermati dopo due tentativi che non hanno funzionato
    [held] aggiornamenti automatici: non attivati · ultimo controllo { $date } · gli aggiornamenti sono trattenuti
    [hold-unknown] aggiornamenti automatici: non attivati · ultimo controllo { $date } · il blocco dell'aggiornamento non si può leggere
    [offline] aggiornamenti automatici: non attivati · ultimo controllo { $date } · non è stato possibile connettersi al servizio di aggiornamento
    [metered] aggiornamenti automatici: non attivati · ultimo controllo { $date } · in attesa di una connessione che non è a consumo
    [waiting-for-signed-installer] aggiornamenti automatici: non attivati · ultimo controllo { $date } · in attesa della prossima versione
    [busy] aggiornamenti automatici: non attivati · ultimo controllo { $date } · un altro aggiornamento era in esecuzione
    [not-eligible] aggiornamenti automatici: non attivati · ultimo controllo { $date } · questa installazione non può aggiornarsi da sola
   *[failed] aggiornamenti automatici: non attivati · ultimo controllo { $date } · l'aggiornamento non ha funzionato ({ $code })
}

journal.cli.sol.update.catalogue.download = non è stato possibile scaricare il catalogo delle versioni.

journal.cli.sol.update.catalogue.key = la chiave della versione in questa build non è leggibile.

journal.cli.sol.update.catalogue.lane = canale di versione sconosciuto: { $lane }

journal.cli.sol.update.catalogue.latest = il marcatore dell'ultima versione è malformato.

journal.cli.sol.update.catalogue.pin = il catalogo delle versioni nomina una chiave della versione diversa.

journal.cli.sol.update.catalogue.release = il catalogo delle versioni non corrisponde alla versione richiesta.

journal.cli.sol.update.catalogue.schema.bad.catalogue.revision = il catalogo delle versioni non è valido: revisione del catalogo errata.

journal.cli.sol.update.catalogue.schema.bad.desktop.version = il catalogo delle versioni non è valido: versione desktop errata.

journal.cli.sol.update.catalogue.schema.bad.installer = il catalogo delle versioni non è valido: programma di installazione errato.

journal.cli.sol.update.catalogue.schema.bad.version = il catalogo delle versioni non è valido: versione errata.

journal.cli.sol.update.catalogue.schema.installer.in.an.unsupported.schema = il catalogo delle versioni non è valido: programma di installazione in uno schema non supportato.

journal.cli.sol.update.catalogue.schema.key.id = il catalogo delle versioni non è valido: id della chiave.

journal.cli.sol.update.catalogue.schema.no.journal.version = il catalogo delle versioni non è valido: nessuna versione del diario.

journal.cli.sol.update.catalogue.schema.no.version = il catalogo delle versioni non è valido: nessuna versione.

journal.cli.sol.update.catalogue.schema.not.json = il catalogo delle versioni non è valido: non è json.

journal.cli.sol.update.catalogue.schema.unsupported.protocol = il catalogo delle versioni non è valido: protocollo non supportato.

journal.cli.sol.update.catalogue.schema.unsupported.schema = il catalogo delle versioni non è valido: schema non supportato.

journal.cli.sol.update.catalogue.signature = il catalogo delle versioni non ha superato il controllo della firma.

journal.cli.sol.update.catalogue.version = la versione della piattaforma richiesta non è valida.

journal.cli.sol.update.receipt.claimed.twice = componente { $component } rivendicato sia dalla ricevuta tree sia dalla ricevuta package

journal.cli.sol.update.receipt.duplicate.roles = i componenti journal e cli sono entrambi registrati

journal.cli.sol.update.receipt.incomplete = la ricevuta di installazione è incompleta o non supportata. usa il programma di installazione originale per recuperarla.

journal.cli.sol.update.receipt.key.duplicate = chiave duplicata { $key }

journal.cli.sol.update.receipt.key.outside = chiave fuori dalla sezione

journal.cli.sol.update.receipt.line.invalid = riga malformata: { $line }

journal.cli.sol.update.receipt.line.pair = non è key=value

journal.cli.sol.update.receipt.native.file = nativo { $receipt_file } non è un file regolare

journal.cli.sol.update.receipt.native.malformed = nativo { $receipt_file } malformato: { $detail }

journal.cli.sol.update.receipt.native.route = la route della ricevuta nativa deve essere tree

journal.cli.sol.update.receipt.native.schema = la ricevuta nativa { $schema_field } deve essere 1

journal.cli.sol.update.receipt.native.version = la versione installata non corrisponde alla ricevuta { $journal_field }

journal.cli.sol.update.receipt.package.file = pacchetto { $config_file } non è un file regolare

journal.cli.sol.update.receipt.package.malformed = pacchetto { $config_file } malformato: { $detail }

journal.cli.sol.update.receipt.section.duplicate = sezione duplicata [{ $section }]

journal.cli.sol.update.receipt.section.key.duplicate = chiave duplicata { $key } nella sezione [{ $section }]

journal.cli.sol.update.receipt.section.unclosed = intestazione di sezione non chiusa

journal.cli.sol.update.receipt.section.unknown = sezione non riconosciuta in { $config_file }: { $section }

journal.cli.sol.update.receipt.unrecorded = questo diario è stato installato da un pacchetto senza il programma di installazione { $brand }, quindi { $update_command } non può ancora aggiornarlo. esegui { $install_command } una volta. controlla i file installati rispetto alla versione firmata, e { $update_command } lo aggiorna dopo.

journal.cli.sol.update.receipt.user.file = utente { $config_file } non è un file regolare

journal.cli.sol.update.receipt.user.malformed = utente { $config_file } malformato: { $detail }

journal.cli.sol.update.report.available = è disponibile un aggiornamento.

journal.cli.sol.update.report.available.package.add = è disponibile un aggiornamento. installato da un pacchetto. per ottenere gli aggiornamenti tramite il programma di aggiornamento del tuo sistema, aggiungi il repository { $brand }: { $package_url }

journal.cli.sol.update.report.available.package.configured = è disponibile un aggiornamento. installato da un pacchetto. gli aggiornamenti arrivano tramite il programma di aggiornamento del tuo sistema.

journal.cli.sol.update.report.catalogue.invalid = i metadati della versione selezionata non sono validi.

journal.cli.sol.update.report.checkout = questa copia è un checkout del sorgente. installa il diario pubblicato per aggiornarlo.

journal.cli.sol.update.report.current = il tuo diario è aggiornato.

journal.cli.sol.update.report.current.package.add = il tuo diario è aggiornato. installato da un pacchetto. per ottenere gli aggiornamenti tramite il programma di aggiornamento del tuo sistema, aggiungi il repository { $brand }: { $package_url }

journal.cli.sol.update.report.current.package.configured = il tuo diario è aggiornato. installato da un pacchetto. gli aggiornamenti arrivano tramite il programma di aggiornamento del tuo sistema.

journal.cli.sol.update.report.held = gli aggiornamenti sono trattenuti. esegui { $unhold_command } per rilasciarli.

journal.cli.sol.update.report.hold.changed = il blocco dell'aggiornamento non si può leggere. non è stato cambiato niente.

journal.cli.sol.update.report.hold.off = gli aggiornamenti non sono più trattenuti.

journal.cli.sol.update.report.hold.on = gli aggiornamenti sono trattenuti.

journal.cli.sol.update.report.hold.save = non è stato possibile salvare il blocco dell'aggiornamento. controlla i permessi della tua directory dei dati.

journal.cli.sol.update.report.hold.unreadable = il blocco dell'aggiornamento non si può leggere.

journal.cli.sol.update.report.install.directory = non è stato possibile identificare la directory di installazione.

journal.cli.sol.update.report.installer.arguments = argomenti del programma di installazione non validi.

journal.cli.sol.update.report.installer.failed = l'installazione non è riuscita. usa il programma di installazione originale per recuperare.

journal.cli.sol.update.report.installer.fetch = non è stato possibile scaricare il programma di installazione.

journal.cli.sol.update.report.installer.run = non è stato possibile eseguire il programma di installazione.

journal.cli.sol.update.report.installer.tookover = aggiornato il tuo diario. ora si aggiorna con il programma di installazione { $brand }, da dove si trova.

journal.cli.sol.update.report.installer.updated = aggiornato il tuo diario.

journal.cli.sol.update.report.linux = { $update_command } è disponibile su linux.

journal.cli.sol.update.report.mode = questa modalità non è un aggiornamento.

journal.cli.sol.update.report.owner.unknown = non è stato possibile identificare come è stata installata questa copia di { $brand }. usa il programma di installazione che l'ha installata per aggiornarla.

journal.cli.sol.update.report.platform.older = non è stato possibile far corrispondere questa build più vecchia a una versione della piattaforma. scegli una versione della piattaforma con { $version_flag }.

journal.cli.sol.update.report.rollback.package = questa route di pacchetto non può installare un diario più vecchio. non è stato cambiato niente.

journal.cli.sol.update.report.unknown = lo stato dell'aggiornamento è sconosciuto.

journal.cli.sol.update.rollback.epoch = installa la versione attuale o un'altra versione in questa epoca di upgrade

journal.cli.sol.update.rollback.link = il link della versione attuale è assente o non è leggibile

journal.cli.sol.update.rollback.metadata = i metadati della versione installata non sono leggibili

journal.cli.sol.update.rollback.no.older = nessuna build più vecchia idonea è conservata. scegli una versione della piattaforma con { $version_flag }, o reinstalla la versione attuale.

journal.cli.sol.update.rollback.policy = la versione attuale non ha un criterio di conservazione supportato

journal.cli.sol.update.rollback.range.absent = la versione installata non ha un intervallo del lettore dichiarato. non è stato cambiato niente.

journal.cli.sol.update.rollback.range.outside = la versione selezionata è fuori dall'intervallo del lettore dichiarato. non è stato cambiato niente.

journal.cli.sol.update.rollback.retained = quella build non è conservata; reinstalla la versione attuale

journal.cli.sol.update.rollback.state = il programma di installazione non può stabilire lo stato del tuo diario. non è stato cambiato niente.

journal.cli.sol.update.rollback.version = la versione di destinazione { $target_version } non è più vecchia di { $installed_version }

journal.cli.sol.update.summary.failure = { $code }: { $message }

journal.cli.sol.update.summary.hold.held = blocco: trattenuto

journal.cli.sol.update.summary.hold.unheld = blocco: non trattenuto

journal.cli.sol.update.summary.hold.unknown = blocco: sconosciuto

journal.cli.sol.update.summary.journal = diario: { $value }

journal.cli.sol.update.summary.lane = canale: { $value }

journal.cli.sol.update.summary.platform = piattaforma: { $value }

journal.cli.sol.update.summary.route = route: { $value }

journal.cli.sol.update.summary.target = disponibile: { $available } (diario { $journal })

journal.cli.sol.usage = Uso: { $syntax }
