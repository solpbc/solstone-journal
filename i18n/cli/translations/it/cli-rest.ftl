journal.cli.backfill.committed = SCRITTO

journal.cli.backfill.day.missing = Il giorno { $day } non è stato trovato nel diario

journal.cli.backfill.days = Non è stato possibile elencare i giorni della cronaca: { $error }

journal.cli.backfill.flags.conflict = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: errore: argomento { $flag }: non consentito con l'argomento { $previous }

journal.cli.backfill.flags.missing = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: errore: argomento { $day_flag }: è atteso un argomento

journal.cli.backfill.flags.unknown = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: errore: argomenti non riconosciuti: { $argument }

journal.cli.backfill.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }Riempi record d'elaborazione vuoti su output nativi{ $line_break }{ $handlers } bloccati, sola intestazione{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $day_option }{ $gap_6 }Giorno in formato { $date_format }; predefinito tutti i giorni{ $line_break }{ $indent_2 }{ $commit_option }{ $gap_7 }Riscrivi intestazioni { $jsonl_format } idonee con record di elaborazione vuoti{ $line_break }{ $indent_2 }{ $dry_run_option }{ $gap_6 }Anteprima output idonei senza scrivere modifiche{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }Attiva output verboso{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Attiva log di debug

journal.cli.backfill.identity.ambiguous = la directory del flusso denominata "{ $default_stream }" non può essere scritta come identità di record: { $path }

journal.cli.backfill.identity.duplicate = più segmenti condividono il flusso { $stream } e la chiave { $key }

journal.cli.backfill.identity.encoding = il percorso del segmento non è rappresentabile in { $encoding }: { $path }

journal.cli.backfill.invalid.day = è atteso un giorno nel formato { $date_format }

journal.cli.backfill.marker = Timbrato { $path }, ma non è stato possibile segnare il giorno { $day } come aggiornato: { $error }

journal.cli.backfill.preview = PROVA A SECCO (nessuna modifica scritta)

journal.cli.backfill.segment.directory = Non è stato possibile elencare la directory dei segmenti { $path }: { $error }

journal.cli.backfill.segments = Non è stato possibile elencare i segmenti del giorno { $day }: { $error }

journal.cli.backfill.stamp.changed = Non è stato possibile timbrare { $path }: il file è cambiato dopo la classificazione

journal.cli.backfill.stamp.read = Non è stato possibile timbrare { $path }: non è stato possibile rileggere il file: { $error }

journal.cli.backfill.stamp.write = Non è stato possibile timbrare { $path }: scrittura non riuscita: { $error }

journal.cli.backfill.stream.directory = Non è stato possibile elencare la directory del flusso { $path }: non è una directory

journal.cli.backfill.stream.encoding = il flusso del segmento non è rappresentabile in { $encoding }: { $path }

journal.cli.brain.arguments.invalid = { $command }: errore: argomenti non validi

journal.cli.brain.busy = Ragionamento occupato: un controllo è già in corso

journal.cli.brain.check.config = controllo ragionamento non riuscito: non è stato possibile leggere config del diario: { $detail }

journal.cli.brain.check.failure = controllo del ragionamento non riuscito: { $detail }

journal.cli.brain.check.fingerprint.options = l'impronta attiva attesa e l'assenza attesa si escludono a vicenda

journal.cli.brain.check.key.missing = il provider ha bisogno di una chiave, e il tuo diario non ha potuto confermare che il risultato fosse salvato.

journal.cli.brain.check.key.rejected = il provider ha rifiutato questa chiave, e il tuo diario non ha potuto confermare che il risultato fosse salvato.

journal.cli.brain.check.open.directory = non è stato possibile aprire il discendente della directory piatta per { $path }: { $reason }

journal.cli.brain.check.random = non è stato possibile generare byte casuali sicuri

journal.cli.brain.heading.blocked = Ragionamento bloccato: { $reason }

journal.cli.brain.heading.blocked.component = Ragionamento bloccato: { $reason } ({ $component })

journal.cli.brain.heading.checking = Controllo del ragionamento: { $reason }

journal.cli.brain.heading.checking.component = Controllo del ragionamento: { $reason } ({ $component })

journal.cli.brain.heading.other = Ragionamento { $state }: { $reason }

journal.cli.brain.heading.other.component = Ragionamento { $state }: { $reason } ({ $component })

journal.cli.brain.heading.unhealthy = Ragionamento non in buono stato: { $reason }

journal.cli.brain.heading.unhealthy.component = Ragionamento non in buono stato: { $reason } ({ $component })

journal.cli.brain.heading.unknown = Ragionamento sconosciuto: { $reason }

journal.cli.brain.heading.unknown.component = Ragionamento sconosciuto: { $reason } ({ $component })

journal.cli.brain.help.refresh = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Emetti { $json_format } invece dell'output semplice{ $line_break }{ $syntax_continuation_7 }{ $line_break }{ $syntax_continuation_8 }{ $line_break }{ $syntax_continuation_9 }

journal.cli.brain.help.renew = uso: { $syntax }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }mostra questo messaggio di aiuto ed esci

journal.cli.brain.help.root = uso: { $syntax }{ $line_break }{ $line_break }Stato ragionamento attivo e CLI di aggiornamento limitato.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $status }Mostra stato ragionamento attivo{ $line_break }    { $refresh }Esegui controllo limitato del ragionamento attivo{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_10 }mostra questo messaggio di aiuto ed esci

journal.cli.brain.help.status = uso: { $syntax }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }Emetti { $json_format } invece dell'output semplice

journal.cli.brain.hold.capacity = la capacità di trattenimento delle credenziali non è disponibile

journal.cli.brain.hold.invalid.provider = provider di trattenimento delle credenziali non supportato

journal.cli.brain.hold.root.changed = la radice del diario ammessa è cambiata

journal.cli.brain.hold.superseded = la validazione delle credenziali è stata sostituita

journal.cli.brain.hold.unavailable = lo stato di trattenimento delle credenziali non è disponibile

journal.cli.brain.ready.byo = Ragionamento pronto: il tuo modello

journal.cli.brain.ready.byo.checked = Ragionamento pronto: il tuo modello, controllato { $age } fa

journal.cli.brain.ready.byo.model = Ragionamento pronto: il tuo modello { $model }

journal.cli.brain.ready.byo.model.checked = Ragionamento pronto: il tuo modello { $model }, controllato { $age } fa

journal.cli.brain.ready.local = Ragionamento pronto: ragionamento locale

journal.cli.brain.ready.local.checked = Ragionamento pronto: ragionamento locale, controllato { $age } fa

journal.cli.brain.ready.local.model = Ragionamento pronto: ragionamento locale { $model }

journal.cli.brain.ready.local.model.checked = Ragionamento pronto: ragionamento locale { $model }, controllato { $age } fa

journal.cli.brain.ready.other = Ragionamento pronto: { $lane }

journal.cli.brain.ready.other.checked = Ragionamento pronto: { $lane }, controllato { $age } fa

journal.cli.brain.ready.other.model = Ragionamento pronto: { $lane } { $model }

journal.cli.brain.ready.other.model.checked = Ragionamento pronto: { $lane } { $model }, controllato { $age } fa

journal.cli.brain.ready.spp = Ragionamento pronto: elaborazione confidenziale

journal.cli.brain.ready.spp.checked = Ragionamento pronto: elaborazione confidenziale, controllata { $age } fa

journal.cli.brain.ready.spp.model = Ragionamento pronto: elaborazione confidenziale { $model }

journal.cli.brain.ready.spp.model.checked = Ragionamento pronto: elaborazione confidenziale { $model }, controllata { $age } fa

journal.cli.brain.ready.unknown = Ragionamento pronto: sconosciuto

journal.cli.brain.ready.unknown.checked = Ragionamento pronto: sconosciuto, controllato { $age } fa

journal.cli.brain.ready.unknown.model = Ragionamento pronto: sconosciuto { $model }

journal.cli.brain.ready.unknown.model.checked = Ragionamento pronto: sconosciuto { $model }, controllato { $age } fa

journal.cli.brain.settlement.fingerprint.changed = la chiave dell'impronta protetta è cambiata dopo la validazione

journal.cli.brain.settlement.publication.changed = la validazione delle credenziali è cambiata dopo la pubblicazione

journal.cli.brain.settlement.root.changed = il percorso del diario ammesso è cambiato

journal.cli.brain.settlement.selection.changed = la selezione delle credenziali è cambiata durante l'aggiornamento

journal.cli.brain.settlement.unconfirmed = la pubblicazione della validazione delle credenziali non è stata confermata

journal.cli.brain.settlement.validation.changed = la validazione delle credenziali è cambiata dopo l'aggiornamento

journal.cli.brain.usage = uso: { $syntax }

journal.cli.brain.validation.checking.aggregate.requires.matching.checking.marker = l'aggregato di controllo richiede un marcatore di controllo corrispondente

journal.cli.brain.validation.checking.reason.is.not.valid.component.evidence = il motivo di controllo non è una prova valida del componente

journal.cli.brain.validation.component.status.does.not.match.reason.aggregate = lo stato del componente non corrisponde all'aggregato dei motivi

journal.cli.brain.validation.detail = { $field }: { $reason }

journal.cli.brain.validation.diagnostic.enum.value.not.allowed = valore enum diagnostico non consentito

journal.cli.brain.validation.diagnostic.key.not.allowed = chiave diagnostica non consentita

journal.cli.brain.validation.diagnostic.value.must.be.an.enum.string = il valore diagnostico deve essere una stringa enum

journal.cli.brain.validation.does.not.match.aggregate.state = non corrisponde a { $token_aggregate_state }

journal.cli.brain.validation.expected.sha256.hex.string = è attesa una stringa esadecimale { $token_sha_256 }

journal.cli.brain.validation.is.required.when.status.is.ok = è obbligatorio quando lo stato è ok

journal.cli.brain.validation.missing.lane.applicable.evidence.without.higher.priority.cause = manca la prova applicabile alla corsia senza una causa a priorità superiore

journal.cli.brain.validation.must.be.a.non.negative.integer = deve essere un intero non negativo

journal.cli.brain.validation.must.be.a.sha256.hex.digest = deve essere un digest esadecimale { $token_sha_256 }

journal.cli.brain.validation.must.be.a.string = deve essere una stringa

journal.cli.brain.validation.must.be.a.string.or.null = deve essere una stringa o null

journal.cli.brain.validation.must.be.an.integer = deve essere un intero

journal.cli.brain.validation.must.be.an.iso8601.timestamp = deve essere un timestamp { $token_iso_8601 }

journal.cli.brain.validation.must.be.an.object = deve essere un oggetto

journal.cli.brain.validation.must.be.evidence.recordable = deve essere scrivibile come prova

journal.cli.brain.validation.must.be.non.empty = deve essere non vuoto

journal.cli.brain.validation.must.be.null.when.aggregate.state.is.ready = deve essere null quando { $token_aggregate_state } è pronto

journal.cli.brain.validation.must.be.provider.key.invalid = deve essere { $token_provider_key_invalid }

journal.cli.brain.validation.must.be.set.when.aggregate.state.is.not.ready = deve essere impostato quando { $token_aggregate_state } non è pronto

journal.cli.brain.validation.non.ok.evidence.requires.reason = la prova non ok richiede un motivo

journal.cli.brain.validation.not.attempted.is.only.valid.for.generate = { $token_not_attempted } è valido solo per generate

journal.cli.brain.validation.not.attempted.reason.must.repeat.causal.prerequisite.reason = il motivo { $token_not_attempted } deve ripetere il motivo del prerequisito causale

journal.cli.brain.validation.not.attempted.requires.a.non.ok.prerequisite = { $token_not_attempted } richiede un prerequisito non ok

journal.cli.brain.validation.ok.evidence.requires.null.reason = la prova ok richiede un motivo null

journal.cli.brain.validation.prerequisite.renewal.requires.ok.or.declared.failure = il rinnovo del prerequisito richiede ok o un fallimento dichiarato

journal.cli.brain.validation.reason.not.allowed.for.evidence.component = motivo non consentito per il componente di prova

journal.cli.brain.validation.record.aggregate.reason.does.not.match.evidence = aggregato/motivo del record non corrisponde alla prova

journal.cli.brain.validation.unknown.field = campo sconosciuto

journal.cli.brain.validation.unknown.top.level.field = campo di primo livello sconosciuto

journal.cli.brain.validation.unknown.value = valore sconosciuto

journal.cli.brain.validation.unsupported.binding.version = versione di binding non supportata

journal.cli.brain.validation.unsupported.schema.version = versione dello schema non supportata

journal.cli.brain.writer.active.brain.fingerprint.is.unavailable = l'impronta del ragionamento attivo non è disponibile

journal.cli.brain.writer.brain.abandon.reason.is.not.recordable.evidence = il motivo di abbandono del ragionamento non è una prova scrivibile

journal.cli.brain.writer.brain.fingerprint.changed = l'impronta del ragionamento è cambiata

journal.cli.brain.writer.brain.fingerprint.key.is.unavailable = la chiave dell'impronta del ragionamento non è disponibile

journal.cli.brain.writer.brain.lane.changed = la corsia del ragionamento è cambiata

journal.cli.brain.writer.brain.prerequisite.evidence.is.unsafe = la prova del prerequisito del ragionamento non è sicura

journal.cli.brain.writer.brain.record.changed.during.inspection = Il record del ragionamento è cambiato durante l'ispezione

journal.cli.brain.writer.brain.refresh.checking.marker.is.absent = il marcatore di controllo dell'aggiornamento del ragionamento è assente

journal.cli.brain.writer.brain.refresh.permit.expired = il permesso di aggiornamento del ragionamento è scaduto

journal.cli.brain.writer.brain.refresh.record.revision.changed = la revisione del record di aggiornamento del ragionamento è cambiata

journal.cli.brain.writer.brain.refresh.revision.changed = la revisione dell'aggiornamento del ragionamento è cambiata

journal.cli.brain.writer.brain.refresh.run.id.changed = l'id di esecuzione aggiornamento ragionamento è cambiato

journal.cli.brain.writer.brain.runtime.failure.marker.changed = il marcatore di fallimento runtime del ragionamento è cambiato

journal.cli.brain.writer.configuration.rejection.changed.after.publication = il rifiuto della configurazione è cambiato dopo la pubblicazione

journal.cli.brain.writer.credential.authority.changed.during.inspection = l'autorità delle credenziali è cambiata durante l'ispezione

journal.cli.brain.writer.credential.authority.changed.during.observation = l'autorità delle credenziali è cambiata durante l'osservazione

journal.cli.brain.writer.credential.authority.is.not.an.object = l'autorità delle credenziali non è un oggetto

journal.cli.brain.writer.credential.authority.is.unreadable = l'autorità delle credenziali non è leggibile

journal.cli.brain.writer.credential.binding.authority.could.not.be.read.error = non è stato possibile leggere l'autorità di binding delle credenziali: { $error }

journal.cli.brain.writer.credential.hold.authority.changed = l'autorità di trattenimento delle credenziali è cambiata

journal.cli.brain.writer.credential.hold.authority.is.not.an.object = l'autorità di trattenimento delle credenziali non è un oggetto

journal.cli.brain.writer.credential.hold.authority.is.unreadable.error = l'autorità di trattenimento delle credenziali non è leggibile: { $error }

journal.cli.brain.writer.credential.observation.authority.is.not.an.object = l'autorità di osservazione delle credenziali non è un oggetto

journal.cli.brain.writer.credential.observation.authority.is.unreadable = l'autorità di osservazione delle credenziali non è leggibile

journal.cli.brain.writer.credential.observation.changed.during.refresh = l'osservazione delle credenziali è cambiata durante l'aggiornamento

journal.cli.brain.writer.credential.observation.changed.during.work = l'osservazione delle credenziali è cambiata durante il lavoro

journal.cli.brain.writer.credential.observation.identity.is.invalid = l'identità dell'osservazione delle credenziali non è valida

journal.cli.brain.writer.credential.observation.lacks.original.binding = all'osservazione delle credenziali manca il binding originale

journal.cli.brain.writer.credential.observations.are.not.an.object = le osservazioni delle credenziali non sono un oggetto

journal.cli.brain.writer.credential.process.state.changed.during.inspection = lo stato del processo delle credenziali è cambiato durante l'ispezione

journal.cli.brain.writer.credential.publication.reservation.is.absent = la riserva di pubblicazione delle credenziali è assente

journal.cli.brain.writer.credential.rejection.changed.after.publication = il rifiuto delle credenziali è cambiato dopo la pubblicazione

journal.cli.brain.writer.credential.rejection.changed.during.refresh = il rifiuto delle credenziali è cambiato durante l'aggiornamento

journal.cli.brain.writer.credential.rejection.changed.during.validation = il rifiuto delle credenziali è cambiato durante la validazione

journal.cli.brain.writer.field.must.be.an.object = { $field } deve essere un oggetto

journal.cli.brain.writer.protected.fingerprint.key.changed.after.rejection = la chiave dell'impronta protetta è cambiata dopo il rifiuto

journal.cli.brain.writer.protected.fingerprint.key.changed.during.admission = la chiave dell'impronta protetta è cambiata durante l'ammissione

journal.cli.brain.writer.protected.fingerprint.key.changed.during.inspection = la chiave dell'impronta protetta è cambiata durante l'ispezione

journal.cli.brain.writer.protected.fingerprint.key.changed.during.validation = la chiave dell'impronta protetta è cambiata durante la validazione

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.a.credential.binding.exists = la chiave dell'impronta protetta è assente mentre esiste un binding di credenziale

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.credential.authority.exists = la chiave dell'impronta protetta è assente mentre esiste l'autorità delle credenziali

journal.cli.brain.writer.protected.fingerprint.key.is.unavailable = la chiave dell'impronta protetta non è disponibile

journal.cli.brain.writer.reason.is.not.recordable.evidence = il motivo non è una prova scrivibile

journal.cli.brain.writer.rejected.refresh.receipt.does.not.match.its.probe = la ricevuta di aggiornamento rifiutata non corrisponde alla sua sonda

journal.cli.brain.writer.rejection.receipt.root.provider.changed = radice/provider della ricevuta di rifiuto cambiati

journal.cli.brain.writer.settled.credential.observation.changed.during.work = l'osservazione consolidata delle credenziali è cambiata durante il lavoro

journal.cli.brain.writer.validation.result.does.not.match.its.credential = il risultato della validazione non corrisponde alla sua credenziale

journal.cli.brain.writer.validation.result.is.not.an.object = il risultato della validazione non è un oggetto

journal.cli.depict.admission = ammissione del processo del diario non riuscita: { $detail }

journal.cli.depict.admission.binding.mismatch = mancata corrispondenza del binding di installazione all'avvio

journal.cli.depict.admission.birth.invalid = la nascita del processo { $linux } ha un { $boot } malformato

journal.cli.depict.admission.birth.position = la nascita del processo { $linux } ha un { $boot } malformato alla riga { $line } colonna { $column }

journal.cli.depict.admission.byte.count = la pipe di avvio ha restituito un conteggio di byte non valido

journal.cli.depict.admission.cleanup.pending = la pulizia { $io } dell'avvio precedente resta incompleta

journal.cli.depict.admission.commit.mismatch = mancata corrispondenza del commit di ammissione all'avvio

journal.cli.depict.admission.deadline.elapsed = termine di avvio scaduto

journal.cli.depict.admission.deadline.invalid = termine di avvio { $windows } scaduto o non valido

journal.cli.depict.admission.descriptor.encoding = descrittore di avvio { $windows } non-{ $unicode }

journal.cli.depict.admission.descriptor.size = descrittore di avvio { $windows } sovradimensionato

journal.cli.depict.admission.endpoint = endpoint di avvio non valido

journal.cli.depict.admission.entry.hosted = attività installata all'entry ospitata

journal.cli.depict.admission.entry.mismatch = mancata corrispondenza della variante di avvio o dell'azione installata

journal.cli.depict.admission.frame.size = dimensione del frame di avvio non valida

journal.cli.depict.admission.grant.access = l'accesso o il tipo dell'handle di avvio non corrisponde alla sua concessione

journal.cli.depict.admission.grant.disk = la concessione di avvio non è un file su disco

journal.cli.depict.admission.grant.duplicate = handle di avvio o tipo di concessione duplicato

journal.cli.depict.admission.grant.ordinary = la concessione di avvio non è un file ordinario su disco

journal.cli.depict.admission.guard.duplicate = guard di installazione: guard di identità duplicato { $field }

journal.cli.depict.admission.guard.encoding = guard di installazione non-{ $unicode }

journal.cli.depict.admission.guard.invalid = guard di installazione: guard di identità non valido { $field }

journal.cli.depict.admission.guard.mismatch = la tupla del guard di avvio differisce dal guard ereditato

journal.cli.depict.admission.guard.missing = guard di installazione: i campi del guard di identità sono incompleti

journal.cli.depict.admission.handle.invalid = handle trasferito non valido

journal.cli.depict.admission.identity.io.local.folder = risolvere la cartella nota { $local_data }: { $error }

journal.cli.depict.admission.identity.io.marker.clone = clonare il marcatore di adozione: { $error }

journal.cli.depict.admission.identity.io.marker.read = leggere il marcatore di adozione: { $error }

journal.cli.depict.admission.identity.io.marker.stat = stat del marcatore di adozione: { $error }

journal.cli.depict.admission.identity.io.namespace.read = leggere la directory del namespace: { $error }

journal.cli.depict.admission.identity.io.owner.open = aprire la directory del titolare: { $error }

journal.cli.depict.admission.identity.io.record.read = leggere il record di identità: { $error }

journal.cli.depict.admission.identity.io.record.stat = stat del record di identità: { $error }

journal.cli.depict.admission.identity.io.root = canonicalizzare la radice: { $error }

journal.cli.depict.admission.identity.io.storage.attrs = interrogare gli attributi di storage: { $error }

journal.cli.depict.admission.identity.io.storage.file = aprire il file di storage: { $error }

journal.cli.depict.admission.identity.io.storage.identity = interrogare l'identità di storage: { $error }

journal.cli.depict.admission.identity.io.storage.lock = acquisire il lock dello storage di identità: { $error }

journal.cli.depict.admission.identity.io.storage.open = aprire la directory di storage: { $error }

journal.cli.depict.admission.identity.local.encoding = la cartella nota { $local_data } non è un { $utf } valido

journal.cli.depict.admission.identity.marker.content = il marcatore di adozione ha un contenuto non valido

journal.cli.depict.admission.identity.marker.missing = il record scritto non ha un marcatore di adozione

journal.cli.depict.admission.identity.marker.orphan = marcatore di adozione orfano

journal.cli.depict.admission.identity.marker.size = il marcatore di adozione ha una dimensione non valida

journal.cli.depict.admission.identity.namespace.child = figlio del namespace sconosciuto

journal.cli.depict.admission.identity.owner.poisoned = il coordinatore del titolare è avvelenato

journal.cli.depict.admission.identity.path.device = i percorsi verbatim e device { $windows } non sono accettati

journal.cli.depict.admission.identity.path.encoding = il percorso { $windows } non è un { $utf } valido

journal.cli.depict.admission.identity.path.escape = il percorso { $windows } esce dalla sua radice assoluta

journal.cli.depict.admission.identity.path.nul = il percorso { $windows } contiene un { $nul } interno

journal.cli.depict.admission.identity.path.relative = il percorso { $windows } deve essere completamente qualificato

journal.cli.depict.admission.identity.path.server = il percorso { $windows } { $unc } non ha un server

journal.cli.depict.admission.identity.path.share = il percorso { $windows } { $unc } non ha una condivisione

journal.cli.depict.admission.identity.path.size = il percorso supera { $limit } byte

journal.cli.depict.admission.identity.prepared = il record di identità dell'installazione è preparato

journal.cli.depict.admission.identity.record.missing = manca il record del namespace

journal.cli.depict.admission.identity.record.size = il record di identità supera i limiti

journal.cli.depict.admission.identity.root.mismatch = il record non vincola la radice richiesta

journal.cli.depict.admission.identity.storage.directory = il percorso di storage non è una directory

journal.cli.depict.admission.identity.storage.file = il percorso di storage non è un file regolare

journal.cli.depict.admission.identity.storage.reparse = il percorso di storage è un reparse point

journal.cli.depict.admission.identity.tombstoned = il record di identità dell'installazione è un tombstone

journal.cli.depict.admission.nonce = nonce dell'endpoint di avvio non valido

journal.cli.depict.admission.obsolete = marcatore di avvio { $windows } obsoleto

journal.cli.depict.admission.offer.mismatch = mancata corrispondenza della provenienza dell'offerta di avvio

journal.cli.depict.admission.parent.mismatch = il server di avvio non è l'esatto genitore diretto

journal.cli.depict.admission.parent.provenance = provenienza di avvio del genitore non valida

journal.cli.depict.admission.peer.unverified = il peer della pipe di avvio non è verificabilmente in vita

journal.cli.depict.admission.process.creation = il { $filetime } di creazione del processo { $windows } corrente non è disponibile

journal.cli.depict.admission.process.signaled = il processo { $windows } corrente è segnalato

journal.cli.depict.admission.provenance = provenienza di avvio non valida

journal.cli.depict.admission.record.ascii = il record di identità non è { $ascii }

journal.cli.depict.admission.record.checksum = mancata corrispondenza del checksum del record di identità

journal.cli.depict.admission.record.field = { $field } del record di identità non valido

journal.cli.depict.admission.record.lines = il record di identità non ha esattamente otto righe { $linefeed }

journal.cli.depict.admission.record.size = il record di identità supera { $size }

journal.cli.depict.admission.root.missing = l'eseguibile di avvio non ha una radice di installazione

journal.cli.depict.admission.server.mismatch = mancata corrispondenza dell'identità del server della pipe di avvio

journal.cli.depict.admission.snapshot.absent = il processo corrente è assente dall'istantanea del genitore

journal.cli.depict.admission.task.provenance = provenienza dell'attività installata non valida

journal.cli.depict.admission.wait.complete = attesa { $io } di avvio non riuscita: { $error }; annullamento { $io } completato

journal.cli.depict.admission.wait.deadline.complete = termine di avvio scaduto; annullamento { $io } completato

journal.cli.depict.admission.wait.deadline.incomplete = termine di avvio scaduto; pulizia { $io } incompleta e conservata

journal.cli.depict.admission.wait.incomplete = attesa { $io } di avvio non riuscita: { $error }; pulizia { $io } incompleta e conservata

journal.cli.depict.detection.unavailable = Il rilevamento degli oggetti non è disponibile. Esegui `{ $command }` per controllare o riparare gli asset { $engine }.

journal.cli.depict.detection.unavailable.windows = il rilevamento degli oggetti non è disponibile. ripara o reinstalla il tuo diario.

journal.cli.depict.help = uso: { $syntax }

journal.cli.depict.metadata.invalid = depict nativo: { $variable } { $format } non valido

journal.cli.describe.cancelled = describe annullato

journal.cli.describe.decode.failed = non è stato possibile decodificare il video: { $path }

journal.cli.describe.deferred = describe differito

journal.cli.describe.deferred.reason = describe differito: { $reason }

journal.cli.describe.faces.skipped = arricchimento dei volti omesso: { $kind }

journal.cli.describe.file.encoding = il percorso del video deve avere un nome di file { $encoding }

journal.cli.describe.output.encode = non è stato possibile codificare l'output: { $error }

journal.cli.describe.output.write = non è stato possibile scrivere l'output: { $error }

journal.cli.describe.path.config = non è stato possibile leggere il percorso del diario: { $reason }

journal.cli.describe.path.home = non è stato possibile risolvere la directory home: { $reason }

journal.cli.describe.pipeline.flat.bound = il nome della directory vincolata esiste e non è una directory

journal.cli.describe.pipeline.flat.create.flat.directory.child = non è stato possibile creare il figlio della directory piatta per { $path }: { $detail }

journal.cli.describe.pipeline.flat.normal = il nome deve essere un solo componente normale

journal.cli.describe.pipeline.flat.read.flat.directory.identity = non è stato possibile leggere l'identità della directory piatta per { $path }: { $detail }

journal.cli.describe.pipeline.flat.relative = il percorso deve essere una sequenza non vuota di componenti normali portabili

journal.cli.describe.pipeline.flat.revalidate.flat.directory = non è stato possibile rivalidare la directory piatta per { $path }: { $detail }

journal.cli.describe.pipeline.flat.stat.flat.directory.descendant = stat del discendente della directory piatta non riuscito per { $path }: { $detail }

journal.cli.describe.pipeline.flat.stat.opened.flat.directory.descendant = stat del discendente aperto della directory piatta non riuscito per { $path }: { $detail }

journal.cli.describe.pipeline.flat.utf8 = il nome non è testo { $encoding } valido

journal.cli.describe.pipeline.frames.failed = tutte le richieste di fotogrammi di describe non sono riuscite

journal.cli.describe.pipeline.install.observe.installed.destination.after.move = { $path }: osservare la destinazione installata dopo lo spostamento: { $detail }

journal.cli.describe.pipeline.install.reconcile.install.after.failed.move = { $path }: riconciliare l'installazione dopo lo spostamento non riuscito: { $detail }

journal.cli.describe.pipeline.install.revalidate.install.paths.after.move = { $path }: rivalidare i percorsi di installazione dopo lo spostamento: { $detail }

journal.cli.describe.pipeline.jobs = { $flag } deve essere positivo

journal.cli.describe.pipeline.merge.frame = manca il fotogramma di unione

journal.cli.describe.pipeline.metadata.object = { $segment_meta } deve essere un oggetto

journal.cli.describe.pipeline.ocr.coverage = la copertura { $ocr } dello schermo è incompleta; i diagnostici privati dei fotogrammi annotano la lacuna

journal.cli.describe.pipeline.ocr.publication = la pubblicazione { $ocr } dello schermo non è riuscita: { $kind }

journal.cli.describe.pipeline.ocr.retire = non è stato possibile ritirare l'aggregato obsoleto { $ocr } dello schermo: { $kind }

journal.cli.describe.pipeline.parent = il video non ha un padre

journal.cli.describe.pipeline.progress = la finestra di fotogrammi di describe non ha fatto progressi

journal.cli.describe.pipeline.publish.anchor.is.not.a.mount.manager.volume.guid.path = l'ancora non è un percorso di volume { $guid } di { $manager }

journal.cli.describe.pipeline.publish.changed = la pubblicazione preparata è cambiata

journal.cli.describe.pipeline.publish.component = la foglia della pubblicazione non è un componente semplice

journal.cli.describe.pipeline.publish.current.directory.is.not.valid.utf = la directory corrente non è { $encoding } valido

journal.cli.describe.pipeline.publish.destination = la destinazione della pubblicazione non è un file semplice

journal.cli.describe.pipeline.publish.destination.already.identifies.the.source.file = la destinazione identifica già il file di origine

journal.cli.describe.pipeline.publish.destination.changed.during.publication = la destinazione è cambiata durante la pubblicazione

journal.cli.describe.pipeline.publish.destination.has.no.normal.name = la destinazione non ha un nome normale

journal.cli.describe.pipeline.publish.destination.is.a.reparse.point = la destinazione è un punto di reparse

journal.cli.describe.pipeline.publish.destination.is.not.a.regular.file = la destinazione non è un file regolare

journal.cli.describe.pipeline.publish.identity = l'identità della pubblicazione preparata è cambiata

journal.cli.describe.pipeline.publish.identity.value.out.of.range = valore di identità fuori intervallo

journal.cli.describe.pipeline.publish.install.publication.exhausted = pubblicazione dell'installazione esaurita

journal.cli.describe.pipeline.publish.leaf = manca la foglia della pubblicazione

journal.cli.describe.pipeline.publish.mode.exceeds.o = la modalità supera { $octal }

journal.cli.describe.pipeline.publish.move.spelling.contains.an.interior.nul = la grafia di spostamento contiene un { $nul } interno

journal.cli.describe.pipeline.publish.parent = manca il padre della pubblicazione

journal.cli.describe.pipeline.publish.path = { $path }: { $detail }

journal.cli.describe.pipeline.publish.publication.path.is.not.valid.utf = il percorso di pubblicazione non è { $encoding } valido

journal.cli.describe.pipeline.publish.published.destination.identity.does.not.match.the.live.source = l'identità della destinazione pubblicata non corrisponde all'origine live

journal.cli.describe.pipeline.publish.published.destination.is.not.a.regular.file = la destinazione pubblicata non è un file regolare

journal.cli.describe.pipeline.publish.published.destination.name.is.absent = il nome della destinazione pubblicata è assente

journal.cli.describe.pipeline.publish.source.and.destination.are.on.different.volumes = l'origine e la destinazione sono su volumi diversi

journal.cli.describe.pipeline.publish.source.and.destination.name.the.same.file = l'origine e la destinazione nominano lo stesso file

journal.cli.describe.pipeline.publish.source.has.no.normal.name = l'origine non ha un nome normale

journal.cli.describe.pipeline.publish.source.identity.changed.before.publication = l'identità dell'origine è cambiata prima della pubblicazione

journal.cli.describe.pipeline.publish.source.identity.is.negative = l'identità dell'origine è negativa

journal.cli.describe.pipeline.publish.source.name.changed.before.publication = il nome dell'origine è cambiato prima della pubblicazione

journal.cli.describe.pipeline.publish.source.name.is.absent.before.publication = il nome dell'origine è assente prima della pubblicazione

journal.cli.describe.pipeline.publish.sync = sincronizzazione del padre della pubblicazione di describe non riuscita: { $error }

journal.cli.describe.pipeline.publish.uncertain = { $path }: { $operation }: { $detail }

journal.cli.describe.pipeline.reappear = il fotogramma { $frame } di describe non è riapparso con la stessa marca temporale

journal.cli.describe.pipeline.record.parent.changed = il padre del testo dello schermo è cambiato

journal.cli.describe.pipeline.record.parent.plain = il padre del testo dello schermo non è una directory semplice

journal.cli.describe.pipeline.reopen = non è stato possibile riaprire il video da describe per materializzare i payload dei fotogrammi

journal.cli.describe.pipeline.resize = non è stato possibile ridimensionare il fotogramma di categorizzazione

journal.cli.describe.pipeline.response.id = la risposta della sessione ha omesso l'id

journal.cli.describe.pipeline.response.unmatched = risposta non correlata

journal.cli.describe.pipeline.retry.binding = associazione del fotogramma nel nuovo tentativo di { $ocr } cambiata

journal.cli.describe.pipeline.retry.evidence.missing = manca la prova del nuovo tentativo

journal.cli.describe.pipeline.retry.handler = il gestore nativo dei fallimenti è cambiato

journal.cli.describe.pipeline.retry.id = l'identità del nuovo tentativo nativo non è { $encoding }

journal.cli.describe.pipeline.retry.incomplete = il nuovo tentativo di { $ocr } non ha raggiunto l'ultimo fotogramma qualificato

journal.cli.describe.pipeline.retry.input.changed = l'input del nuovo tentativo è cambiato durante la lettura

journal.cli.describe.pipeline.retry.input.regular = l'input del nuovo tentativo non è un file regolare

journal.cli.describe.pipeline.retry.io.allocate = non è stato possibile allocare la preparazione

journal.cli.describe.pipeline.retry.io.dest.namespace = lo spazio dei nomi della destinazione è cambiato prima della pubblicazione

journal.cli.describe.pipeline.retry.io.file.kind = l'elemento non è un file regolare

journal.cli.describe.pipeline.retry.io.file.reparse = l'elemento è un punto di reparse

journal.cli.describe.pipeline.retry.io.fsync = il fsync della directory padre è degradato per { $path }: { $error }

journal.cli.describe.pipeline.retry.io.parent.identity = l'identità del padre vincolato è cambiata prima della pubblicazione

journal.cli.describe.pipeline.retry.io.parent.kind = il padre non è una directory reale

journal.cli.describe.pipeline.retry.io.parent.rename = il pathname del padre è cambiato prima della pubblicazione

journal.cli.describe.pipeline.retry.io.parent.reparse = il padre è un punto di reparse

journal.cli.describe.pipeline.retry.io.prepare = l'identità della preparazione è cambiata durante la preparazione

journal.cli.describe.pipeline.retry.io.stage.identity = l'identità della preparazione è cambiata prima della pubblicazione

journal.cli.describe.pipeline.retry.io.stage.namespace = lo spazio dei nomi della preparazione è cambiato prima della pubblicazione

journal.cli.describe.pipeline.retry.io.tmp = non è stato possibile allocare un file temporaneo univoco

journal.cli.describe.pipeline.retry.journal = il diario del nuovo tentativo nativo non corrisponde

journal.cli.describe.pipeline.retry.open = non è stato possibile riaprire il video per il nuovo tentativo di { $ocr }

journal.cli.describe.pipeline.retry.permission.changed = il permesso del nuovo tentativo nativo è cambiato

journal.cli.describe.pipeline.retry.permission.missing = manca il permesso del nuovo tentativo nativo

journal.cli.describe.pipeline.root.canonicalize.journal.root = canonicalizzare la radice del diario: { $detail }

journal.cli.describe.pipeline.root.cloud.files.sync.root.status.could.not.be.verified = non è stato possibile verificare lo stato della radice di sincronizzazione di { $cloud_files }

journal.cli.describe.pipeline.root.filesystem.is.not.ntfs.or.refs = il file system non è { $ntfs } né { $refs }

journal.cli.describe.pipeline.root.filesystem.type.could.not.be.verified = non è stato possibile verificare il tipo di file system

journal.cli.describe.pipeline.root.journal.root.contains.a.reparse.point = la radice del diario contiene un punto di reparse

journal.cli.describe.pipeline.root.journal.root.contains.an.interior.nul = la radice del diario contiene un { $nul } interno

journal.cli.describe.pipeline.root.journal.root.has.an.invalid.portable.name = la radice del diario ha un nome portabile non valido

journal.cli.describe.pipeline.root.journal.root.is.a.cloud.files.sync.root = la radice del diario è una radice di sincronizzazione di { $cloud_files }

journal.cli.describe.pipeline.root.journal.root.is.a.reparse.point = la radice del diario è un punto di reparse

journal.cli.describe.pipeline.root.journal.root.is.on.a.non.local.volume = la radice del diario è su un volume non locale

journal.cli.describe.pipeline.root.journal.root.is.on.a.ram.disk = la radice del diario è su un disco { $ram }

journal.cli.describe.pipeline.root.journal.root.is.on.removable.or.optical.media = la radice del diario è su un supporto rimovibile o ottico

journal.cli.describe.pipeline.root.journal.root.must.be.lexically.canonical = la radice del diario deve essere lessicalmente canonica

journal.cli.describe.pipeline.root.journal.root.must.use.a.fully.qualified.drive.path = la radice del diario deve usare un percorso di unità pienamente qualificato

journal.cli.describe.pipeline.root.journal.root.volume.type.is.unknown = il tipo di volume della radice del diario è sconosciuto

journal.cli.describe.pipeline.root.open.filesystem.root = aprire la radice del file system: { $detail }

journal.cli.describe.pipeline.root.open.journal.directory = aprire la directory del diario: { $detail }

journal.cli.describe.pipeline.root.open.journal.root = aprire la radice del diario: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.ancestor = aprire l'antenato della radice del diario: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.namespace.watch = aprire il watch dello spazio dei nomi della radice del diario: { $detail }

journal.cli.describe.pipeline.root.open.verified.drive.root = aprire la radice di unità verificata: { $detail }

journal.cli.describe.pipeline.root.open.verified.journal.root = aprire la radice del diario verificata: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.ancestor.attributes = interrogare gli attributi dell'antenato della radice del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.attributes = interrogare gli attributi della radice del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.drive.type = interrogare il tipo di unità della radice del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.identity = interrogare l'identità della radice del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.attributes = interrogare gli attributi del watch dello spazio dei nomi della radice del diario: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.identity = interrogare l'identità del watch dello spazio dei nomi della radice del diario: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.attributes = interrogare gli attributi della radice del diario conservata: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.identity = interrogare l'identità della radice del diario conservata: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.attributes = interrogare gli attributi della radice di unità verificata: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.identity = interrogare l'identità della radice di unità verificata: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.attributes = interrogare gli attributi della radice del diario verificata: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.identity = interrogare l'identità della radice del diario verificata: { $detail }

journal.cli.describe.pipeline.root.read.source.file.identity = leggere l'identità del file di origine: { $detail }

journal.cli.describe.pipeline.root.restat.acquired.journal.root = rifare stat della radice del diario acquisita: { $detail }

journal.cli.describe.pipeline.root.stat.acquired.journal.root = fare stat della radice del diario acquisita: { $detail }

journal.cli.describe.pipeline.root.stat.canonical.root.component = fare stat del componente canonico della radice: { $detail }

journal.cli.describe.pipeline.root.stat.opened.canonical.root.component = fare stat del componente canonico aperto della radice: { $detail }

journal.cli.describe.pipeline.root.stat.opened.filesystem.root = fare stat della radice aperta del file system: { $detail }

journal.cli.describe.pipeline.row.frame = la riga di describe ha omesso { $frame_key }

journal.cli.describe.pipeline.source.bytes = i byte di origine di { $ocr } sono cambiati

journal.cli.describe.pipeline.source.changed = l'origine di { $ocr } è cambiata prima della pubblicazione

journal.cli.describe.pipeline.source.identity = l'identità dell'origine di { $ocr } è cambiata

journal.cli.describe.pipeline.source.open = l'origine di { $ocr } è cambiata durante l'apertura

journal.cli.describe.pipeline.source.plain = l'origine di { $ocr } non è un file semplice

journal.cli.describe.pipeline.temp = non è stato possibile allocare il file temporaneo di describe

journal.cli.describe.pipeline.timestamp = il fotogramma { $frame } di describe ha omesso la marca temporale

journal.cli.describe.pipeline.windows.cleanup = { $detail }; non è stato possibile rimuovere la preparazione { $stage }: { $error }

journal.cli.describe.pipeline.windows.name.alternate.data.stream = il nome contiene i due punti

journal.cli.describe.pipeline.windows.name.control = il nome contiene un carattere di controllo

journal.cli.describe.pipeline.windows.name.dot.component = '.' e '..' non sono nomi validi

journal.cli.describe.pipeline.windows.name.empty = il nome è vuoto

journal.cli.describe.pipeline.windows.name.forbidden.character = il nome contiene un carattere che { $windows } non consente

journal.cli.describe.pipeline.windows.name.reserved.device = il nome è riservato da { $windows }

journal.cli.describe.pipeline.windows.name.root.or.prefix = il nome deve essere un solo componente relativo

journal.cli.describe.pipeline.windows.name.separator = il nome contiene un separatore di percorso

journal.cli.describe.pipeline.windows.name.too.long = il nome è più lungo di { $length_limit } byte { $encoding }

journal.cli.describe.pipeline.windows.name.trailing.dot.or.space = il nome termina con un punto o uno spazio

journal.cli.describe.pipeline.windows.path.already.exists = l'antenato '{ $component }' del percorso di pubblicazione è comparso durante la sola creazione

journal.cli.describe.pipeline.windows.path.classify.install.source = classificare l'origine dell'installazione: { $detail }

journal.cli.describe.pipeline.windows.path.component = il componente del percorso di pubblicazione non è portabile: { $detail }

journal.cli.describe.pipeline.windows.path.create.publication.path.ancestor = creare l'antenato del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.dot.component = il percorso di pubblicazione contiene '.' o '..'

journal.cli.describe.pipeline.windows.path.drive.relative = il percorso di pubblicazione è relativo all'unità

journal.cli.describe.pipeline.windows.path.empty = il percorso di pubblicazione è vuoto

journal.cli.describe.pipeline.windows.path.identity.changed = l'identità del percorso di pubblicazione è cambiata al livello conservato { $level }

journal.cli.describe.pipeline.windows.path.identity.mismatch = l'identità dell'antenato '{ $component }' del percorso di pubblicazione è cambiata tra la creazione e la riapertura

journal.cli.describe.pipeline.windows.path.interior.nul = il percorso di pubblicazione contiene un { $nul } interno

journal.cli.describe.pipeline.windows.path.missing = il componente '{ $component }' del percorso di pubblicazione non esiste

journal.cli.describe.pipeline.windows.path.move.spelling.diverged = la grafia di spostamento del percorso di pubblicazione non nomina più il padre terminale

journal.cli.describe.pipeline.windows.path.not.directory = l'antenato '{ $component }' del percorso di pubblicazione non è una directory

journal.cli.describe.pipeline.windows.path.not.regular.file = la foglia '{ $component }' del percorso di pubblicazione non è un file regolare

journal.cli.describe.pipeline.windows.path.open.install.source = aprire l'origine dell'installazione: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.ancestor = aprire l'antenato del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.anchor = aprire l'ancora del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.move.spelling = aprire la grafia di spostamento del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.path.too.long = il percorso di pubblicazione supera il limite di lunghezza estesa

journal.cli.describe.pipeline.windows.path.query.install.path.attributes = interrogare gli attributi del percorso di installazione: { $detail }

journal.cli.describe.pipeline.windows.path.query.install.source.identity = interrogare l'identità dell'origine dell'installazione: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.attributes = interrogare gli attributi del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.identity = interrogare l'identità del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.reopen.publication.path.ancestor = riaprire l'antenato del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.reparse.point = l'antenato '{ $component }' del percorso di pubblicazione è un punto di reparse

journal.cli.describe.pipeline.windows.path.resolve.publication.path.current.directory = risolvere la directory corrente del percorso di pubblicazione: { $detail }

journal.cli.describe.pipeline.windows.path.revalidate.install.source.after.classification = rivalidare l'origine dell'installazione dopo la classificazione: { $detail }

journal.cli.describe.pipeline.windows.path.root.only = il percorso di pubblicazione è una radice di unità senza destinazione

journal.cli.describe.pipeline.windows.path.rooted.without.drive = il percorso di pubblicazione è radicato senza un'unità

journal.cli.describe.pipeline.windows.path.unc.or.device.prefix = il percorso di pubblicazione ha un prefisso { $unc } o di dispositivo

journal.cli.describe.pipeline.windows.path.unstable = il percorso di pubblicazione non ha un volume { $guid } di { $manager }: { $detail }

journal.cli.describe.pipeline.withdraw = non è stato possibile preparare il ritiro { $ocr } dello schermo: { $detail }

journal.cli.describe.settings.finite = { $setting } deve essere un numero finito

journal.cli.describe.settings.importance = { $setting } deve essere { $ignore }, { $low }, { $normal } o { $high }

journal.cli.describe.settings.object = { $setting } deve essere un oggetto

journal.cli.describe.settings.string = { $setting } deve essere una stringa

journal.cli.describe.settings.strings = { $setting } deve essere un array di stringhe

journal.cli.describe.settings.uint = { $setting } deve essere un intero a 32 bit senza segno

journal.cli.describe.stored.error.empty = Risposta markdown vuota per { $category }

journal.cli.describe.stored.error.incomplete = Estrazione mai completata

journal.cli.describe.stored.error.invalid = Risposta { $format } non valida

journal.cli.describe.stored.error.invalid.category = Risposta { $format } non valida per { $category }: { $error }

journal.cli.describe.stored.error.resize = non è stato possibile ridimensionare il fotogramma di estrazione

journal.cli.describe.stored.error.schema = Validazione dello schema non riuscita per { $category }

journal.cli.describe.stored.error.schema.detail = Validazione dello schema non riuscita per { $category }: { $errors }

journal.cli.describe.stored.error.truncated = Risposta markdown troncata per { $category }

journal.cli.describe.stored.note.bounded = [{ $mark }: output dell'estrazione limitato prima della scrittura nel diario - lunghezza degenere sanificata/troncata]

journal.cli.describe.stored.note.ocr = testo riconosciuto dai tuoi schermi. può contenere errori di riconoscimento. questo dice ciò che era visibile, non ciò che hai letto, digitato o fatto. mascherare password e altri segreti è best effort; le tue regole di oscuramento non si applicano.

journal.cli.describe.stored.ocr.heading = { $clock } · { $category }

journal.cli.describe.usage.argument.unknown = uso: { $syntax }{ $line_break }{ $command }: errore: argomento sconosciuto: { $argument }

journal.cli.describe.usage.backfill.arguments = uso: { $syntax }{ $line_break }{ $command }: errore: il riempimento dei volti richiede { $arguments }

journal.cli.describe.usage.backfill.budget = uso: { $syntax }{ $line_break }{ $command }: errore: il budget del riempimento dei volti è fuori dall'intervallo consentito

journal.cli.describe.usage.backfill.desktop = uso: { $syntax }{ $line_break }{ $command }: errore: il riempimento dei volti è disponibile solo sui diari desktop

journal.cli.describe.usage.describe.repeated = uso: { $syntax }{ $line_break }{ $command }: errore: { $describe_flag } è stato fornito più di una volta

journal.cli.describe.usage.file.extra = uso: { $syntax }{ $line_break }{ $command }: errore: si può fornire un solo percorso video

journal.cli.describe.usage.file.required = uso: { $syntax }{ $line_break }{ $command }: errore: i seguenti argomenti sono obbligatori: { $file }

journal.cli.describe.usage.frames.repeated = uso: { $syntax }{ $line_break }{ $command }: errore: { $frames_only_flag } è stato fornito più di una volta

journal.cli.describe.usage.help = uso: { $syntax }

journal.cli.describe.usage.help.extra = uso: { $syntax }{ $line_break }{ $command }: errore: { $help_flag } non accetta altri argomenti

journal.cli.describe.usage.jobs.positive = uso: { $syntax }{ $line_break }{ $command }: errore: { $jobs_flag } richiede un intero positivo

journal.cli.describe.usage.jobs.repeated = uso: { $syntax }{ $line_break }{ $command }: errore: { $jobs_flag } è stato fornito più di una volta

journal.cli.describe.usage.journal.path = uso: { $syntax }{ $line_break }{ $command }: errore: { $journal_flag } richiede un percorso

journal.cli.describe.usage.journal.repeated = uso: { $syntax }{ $line_break }{ $command }: errore: { $journal_flag } è stato fornito più di una volta

journal.cli.describe.usage.mode.required = uso: { $syntax }{ $line_break }{ $command }: errore: è obbligatorio esattamente uno tra { $frames_only_flag } o { $describe_flag }

journal.cli.describe.usage.redo.mode = uso: { $syntax }{ $line_break }{ $command }: errore: { $redo_flag } richiede { $describe_flag }

journal.cli.describe.usage.redo.repeated = uso: { $syntax }{ $line_break }{ $command }: errore: { $redo_flag } è stato fornito più di una volta

journal.cli.describe.usage.registry.extra = uso: { $syntax }{ $line_break }{ $command }: errore: { $category_registry_flag } non accetta altri argomenti

journal.cli.describe.usage.version.extra = uso: { $syntax }{ $line_break }{ $command }: errore: { $version_flag } non accetta altri argomenti

journal.cli.grab.alternatives.days = Giorni disponibili (i 5 più vicini):

journal.cli.grab.alternatives.screens = Schermi disponibili in { $day }/{ $stream }/{ $segment }:

journal.cli.grab.alternatives.segments = Segmenti disponibili in { $day }/{ $stream }:

journal.cli.grab.alternatives.streams = Flussi disponibili in { $day }:

journal.cli.grab.error.captured.not.analyzed = lo schermo { $screen } in { $segment } è preso ma non analizzato

journal.cli.grab.error.day.not.found = giorno { $day } non trovato

journal.cli.grab.error.decode.failed.ids = non è stato possibile decodificare gli id di fotogramma: { $ids }

journal.cli.grab.error.decode.invalid.rgb = non è stato possibile decodificare il video: fotogramma RGB non valido

journal.cli.grab.error.decode.no.video = non è stato possibile decodificare il video: nessun flusso video

journal.cli.grab.error.decode.video = non è stato possibile decodificare il video: { $error }

journal.cli.grab.error.decoder.init = non è stato possibile inizializzare il decodificatore video: { $error }

journal.cli.grab.error.force.requires.out = { $force_flag } richiede { $out_flag }

journal.cli.grab.error.frame.id.positive = gli id di fotogramma devono essere interi positivi: ottenuto '{ $token }'

journal.cli.grab.error.frame.id.unique = gli id di fotogramma devono essere univoci: { $frame_id }

journal.cli.grab.error.frame.not.found = id di fotogramma { $frame_id } non trovato in { $screen } per { $segment }

journal.cli.grab.error.invalid.frame.id = id di fotogramma non valido

journal.cli.grab.error.invalid.level4 = payload di livello 4 non valido

journal.cli.grab.error.invalid.level5a = payload di livello 5a non valido

journal.cli.grab.error.invalid.render.payload = payload di render non valido

journal.cli.grab.error.invalid.segment.key = il segmento { $segment } non è una chiave { $format } valida

journal.cli.grab.error.io = non è stato possibile accedere a { $path }: { $error }

journal.cli.grab.error.legacy.schema = il file dello schermo usa lo schema pre-frame_id; la selezione dei fotogrammi non è disponibile

journal.cli.grab.error.multiple.ids.require.out = più id di fotogramma richiedono { $out_flag }

journal.cli.grab.error.out.exists = il percorso di output esiste (usa { $force_flag }): { $paths }

journal.cli.grab.error.out.extension = { $out_flag } deve terminare con { $extensions }

journal.cli.grab.error.out.filename = il percorso di output non ha un nome di file valido

journal.cli.grab.error.out.requires.five.tokens = { $out_flag } richiede day stream segment screen e frame-id

journal.cli.grab.error.save.image = non è stato possibile salvare l'immagine: { $error }

journal.cli.grab.error.screen.not.found = schermo { $screen } non trovato in { $day }/{ $stream }/{ $segment }

journal.cli.grab.error.segment.not.found = segmento { $segment } non trovato in { $day }/{ $stream }

journal.cli.grab.error.stream.not.found = flusso { $stream } non trovato in { $day }

journal.cli.grab.error.timestamp.finite = il timestamp deve essere finito

journal.cli.grab.error.timestamp.invalid = timestamp non valido

journal.cli.grab.error.timestamp.range = il timestamp è fuori intervallo

journal.cli.grab.error.too.many.tokens = grab accetta al massimo 5 token posizionali: day stream segment screen frame-id

journal.cli.grab.error.unsupported.level = livello di output non supportato

journal.cli.grab.error.unsupported.level.named = livello di output non supportato: { $level }

journal.cli.grab.error.video.missing = video grezzo non trovato per lo schermo { $screen } in { $segment }

journal.cli.grab.error.video.missing.with.reason = { $reason }; l'accesso ai soli metadati resta tramite: { $command }

journal.cli.grab.frame.note.error = errore: { $text }

journal.cli.grab.header.abs.time = abs_time

journal.cli.grab.header.connector = connettore

journal.cli.grab.header.day = giorno

journal.cli.grab.header.end = fine

journal.cli.grab.header.frame.id = frame_id

journal.cli.grab.header.frames.analyzed = frames_analyzed

journal.cli.grab.header.notes = note

journal.cli.grab.header.position = posizione

journal.cli.grab.header.primary = primario

journal.cli.grab.header.screen = schermo

journal.cli.grab.header.screens = schermi

journal.cli.grab.header.segment = segmento

journal.cli.grab.header.segments = segmenti

journal.cli.grab.header.start = inizio

journal.cli.grab.header.status = stato

journal.cli.grab.header.stream = flusso

journal.cli.grab.header.streams = flussi

journal.cli.grab.header.timestamp = timestamp

journal.cli.grab.level4.empty = Nessun fotogramma qualificato nell'analisi di questo schermo.

journal.cli.grab.level4.footer = Ispeziona:   { $inspect_command }{ $line_break }Salva uno:   { $save_one_command }{ $line_break }Salva molti: { $save_many_command }{ $line_break }{ $line_break }Come funziona l'estrazione:{ $line_break }  La decodifica percorre il video in modo lineare dal fotogramma 0 — il salto è insicuro alla{ $line_break }  frequenza di presa di 1 Hz. Il costo è dominato dal frame_id richiesto più alto, non{ $line_break }  dal numero. Chiedere gli id 7,12,23 costa come chiedere solo il 23.{ $line_break }  Preferisci la modalità lotto quando vuoi più di un fotogramma dallo stesso schermo.

journal.cli.grab.level4.legacy = 0 fotogrammi analizzati: il file usa lo schema pre-frame_id

journal.cli.grab.level4.missing.video = Modalità di salvataggio non disponibile: { $reason }.{ $line_break }I metadati dei fotogrammi sopra restano leggibili.{ $line_break }{ $line_break }Ispeziona: { $command }

journal.cli.grab.level5a.footer = Salva: { $save_command }{ $line_break }Lotto: { $batch_command }

journal.cli.grab.level5a.frame = Fotogramma: { $value }

journal.cli.grab.level5a.jsonl = JSONL: { $value }

journal.cli.grab.level5a.notes = Note: { $value }

journal.cli.grab.level5a.screen = Schermo: { $value }

journal.cli.grab.level5a.time = Ora: { $value }

journal.cli.grab.level5a.video = Video: { $value }

journal.cli.grab.missing.reason.offload = hai eliminato il video originale dopo che il tuo backup lo ha copiato

journal.cli.grab.missing.reason.owner = hai eliminato il video originale

journal.cli.grab.missing.reason.policy = hai eliminato il video originale dopo che le tue impostazioni di conservazione lo hanno contrassegnato

journal.cli.grab.missing.reason.trash = il video originale è nel cestino, dove puoi ripristinarlo nelle impostazioni

journal.cli.grab.missing.reason.unknown = il video originale non è più nel tuo diario

journal.cli.grab.next = Poi: { $command }

journal.cli.grab.saved = salvato { $path }

journal.cli.grab.status.analyzed = analizzato

journal.cli.grab.status.captured = preso ma non analizzato

journal.cli.grab.status.missing.offload = analizzato; hai eliminato il video originale dopo che il tuo backup lo ha copiato

journal.cli.grab.status.missing.owner = analizzato; hai eliminato il video originale

journal.cli.grab.status.missing.policy = analizzato; hai eliminato il video originale dopo che le tue impostazioni di conservazione lo hanno contrassegnato

journal.cli.grab.status.missing.trash = analizzato; il video originale è nel cestino

journal.cli.grab.status.missing.unknown = analizzato; il video originale non è più nel tuo diario

journal.cli.maintenance.backup.erase = cancellazione backup: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending }

journal.cli.maintenance.backup.erase.error.detail = cancellazione backup: errore { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.erase.reason = cancellazione backup: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending } { $reason_field }{ $reason }

journal.cli.maintenance.backup.error = backup: errore { $reason_field }{ $reason }

journal.cli.maintenance.backup.error.detail = backup: errore { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.ok = backup: ok { $field }{ $snapshot }

journal.cli.maintenance.backup.paused = backup: saltato. il backup è in pausa, quindi non è stato eseguito nulla. esegui `{ $resume }` per avviarlo di nuovo.

journal.cli.maintenance.backup.prune.error = pulizia backup: errore { $reason_field }{ $reason }

journal.cli.maintenance.backup.prune.error.detail = pulizia backup: errore { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.prune.ok = pulizia backup: ok

journal.cli.maintenance.backup.prune.paused = pulizia backup: saltata. il backup è in pausa, quindi non è stato eseguito nulla. esegui `{ $resume }` per avviarlo di nuovo.

journal.cli.maintenance.backup.prune.skipped = pulizia backup: saltata

journal.cli.maintenance.backup.skipped = backup: saltato

journal.cli.maintenance.backup.superseded = backup: rivendicato da un altro dispositivo; le impostazioni locali sono state cancellate. esegui `{ $enable }` per configurare un nuovo backup qui.

journal.cli.maintenance.backup.unavailable = i servizi di manutenzione del backup non sono disponibili.

journal.cli.maintenance.backup.usage.offload = uso: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: errore: argomenti non riconosciuti: { $args }

journal.cli.maintenance.backup.usage.routine = uso: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: errore: argomenti non riconosciuti: { $args }

journal.cli.maintenance.backup.verify.error = verifica backup: errore { $reason_field }{ $reason }

journal.cli.maintenance.backup.verify.error.detail = verifica backup: errore { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.verify.ok = verifica backup: ok { $field }{ $subset }

journal.cli.maintenance.backup.verify.paused = verifica backup: saltata. il backup è in pausa, quindi non è stato eseguito nulla. esegui `{ $resume }` per avviarlo di nuovo.

journal.cli.maintenance.backup.verify.skipped = verifica backup: saltata

journal.cli.maintenance.description.backup.erase = rimuovi anche dai tuoi backup ciò che hai cancellato, quando lo hai scelto.

journal.cli.maintenance.description.backup.offload = trasferisci i media grezzi verificati dopo il backup.

journal.cli.maintenance.description.backup.prune = applica la politica di conservazione del backup cifrato.

journal.cli.maintenance.description.backup.run = esegui il backup cifrato.

journal.cli.maintenance.description.backup.verify = verifica la lettura di ritorno del backup cifrato.

journal.cli.maintenance.description.faces.backfill = aggiorna i suggerimenti per la foto del profilo dalle riunioni, comprese quelle passate.

journal.cli.maintenance.description.health.mark.raw = elenca i media originali pronti per la rimozione.

journal.cli.maintenance.description.health.prune.logs = pulisci i vecchi log operativi.

journal.cli.maintenance.description.retention.owner.voice.originals = finisci di eliminare l'audio originale delle registrazioni recenti elaborate con "lascia fuori le altre voci" attivo.

journal.cli.maintenance.description.speakers.backfill.labels = nomina i parlanti, senza il modello, nell'audio che non ha ancora etichette di parlante.

journal.cli.maintenance.description.speakers.backfill.pool = completa la lista delle voci dall'audio già trascritto, poi cerca la tua voce.

journal.cli.maintenance.description.speakers.candidate.pair.suggestions = Trova coppie dense di candidati a parlante per Suggerimenti.

journal.cli.maintenance.description.speakers.consolidate.pool = Consolida i candidati a parlante densi.

journal.cli.maintenance.description.speakers.discover.voices = Aggiorna la cache di scoperta delle voci ricorrenti.

journal.cli.maintenance.description.speakers.name.variants = Trova le varianti di nome del parlante per Suggerimenti.

journal.cli.maintenance.description.trash.erase = finisci i passaggi del cestino che un arresto ha interrotto e cancella ciò che ha atteso i suoi giorni.

journal.cli.maintenance.face.generation = la generazione del riempimento dei volti non è stata ammessa

journal.cli.maintenance.face.log.cache = cache di scansione del riempimento dei volti non salvata: { $detail }

journal.cli.maintenance.face.log.deferred = recupero della partecipazione dei volti rimandato: { $detail }

journal.cli.maintenance.face.log.failed = riempimento dei volti non riuscito: { $detail }

journal.cli.maintenance.face.priority = priorità della cronologia dei volti non disponibile: { $error }

journal.cli.maintenance.face.reconciliation = riconciliazione dei volti: { $error }

journal.cli.maintenance.health.config.days = { $field } deve essere un intero positivo

journal.cli.maintenance.health.config.enabled = { $field } deve essere un booleano

journal.cli.maintenance.health.keep = { $routine }: le tue impostazioni di conservazione conservano tutti i media originali.

journal.cli.maintenance.health.mark.entry = { $indent }{ $id }: { $reason }

journal.cli.maintenance.health.mark.refused = { $routine }: non è stato possibile elencare alcuni elementi:

journal.cli.maintenance.health.mark.summary = { $routine }: elementi nuovi: { $new }.{ $line_break }  totale attuale: { $total }.

journal.cli.maintenance.health.mark.unavailable = { $routine }: non è stato possibile costruire l'elenco: { $error }

journal.cli.maintenance.health.prune.args.integer = uso: { $command } { $syntax }{ $line_break }{ $command }: errore: argomento { $flag }: valore intero non valido: '{ $value }'

journal.cli.maintenance.health.prune.args.missing = uso: { $command } { $syntax }{ $line_break }{ $command }: errore: argomento { $flag }: è atteso un argomento

journal.cli.maintenance.health.prune.args.positive = { $routine }: { $flag } deve essere un intero positivo

journal.cli.maintenance.health.prune.args.unexpected = uso: { $command } { $syntax }{ $line_break }{ $command }: errore: argomenti non riconosciuti: { $value }

journal.cli.maintenance.health.prune.class = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } file, { $dirs } directory di cache, { $bytes }
        [many] { $indent }{ $class }: { $files } file, { $dirs } di directory di cache, { $bytes }
       *[other] { $indent }{ $class }: { $files } file, { $dirs } directory di cache, { $bytes }
    }
    [many] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } di file, { $dirs } directory di cache, { $bytes }
        [many] { $indent }{ $class }: { $files } di file, { $dirs } di directory di cache, { $bytes }
       *[other] { $indent }{ $class }: { $files } di file, { $dirs } directory di cache, { $bytes }
    }
   *[other] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } file, { $dirs } directory di cache, { $bytes }
        [many] { $indent }{ $class }: { $files } file, { $dirs } di directory di cache, { $bytes }
       *[other] { $indent }{ $class }: { $files } file, { $dirs } directory di cache, { $bytes }
    }
}

journal.cli.maintenance.health.prune.disabled = { $routine }: disattivato

journal.cli.maintenance.health.prune.halted = { $routine }: { $message }

journal.cli.maintenance.health.prune.missing = la directory del diario non è disponibile

journal.cli.maintenance.health.prune.preview = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: pulirebbe { $files } file di log operativo, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: pulirebbe { $files } file di log operativo, { $dirs } di directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: pulirebbe { $files } file di log operativo, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
    }
    [many] { $dirs_quantity ->
        [one] { $routine }: pulirebbe { $files } di file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: pulirebbe { $files } di file di log operativi, { $dirs } di directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: pulirebbe { $files } di file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: pulirebbe { $files } file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: pulirebbe { $files } file di log operativi, { $dirs } di directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: pulirebbe { $files } file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.queue = un log è stato rimosso, ma non è stato possibile accodare il suo aggiornamento di ricerca: { $error }

journal.cli.maintenance.health.prune.refused = { $routine }: non è stato possibile pulire alcuni log: { $summary }

journal.cli.maintenance.health.prune.refused.unnamed = { $routine }: non è stato possibile pulire alcuni log: lo strumento di conservazione ha rifiutato senza nominare un elemento.

journal.cli.maintenance.health.prune.result = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: ha pulito { $files } file di log operativo, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: ha pulito { $files } file di log operativo, { $dirs } di directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: ha pulito { $files } file di log operativo, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
    }
    [many] { $dirs_quantity ->
        [one] { $routine }: ha pulito { $files } di file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: ha pulito { $files } di file di log operativi, { $dirs } di directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: ha pulito { $files } di file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: ha pulito { $files } file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
        [many] { $routine }: ha pulito { $files } file di log operativi, { $dirs } di directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
       *[other] { $routine }: ha pulito { $files } file di log operativi, { $dirs } directory di cache, { $bytes } { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.unavailable = { $routine }: non è stato possibile pulire i log: { $error }

journal.cli.maintenance.health.prune.undateable = { $indent }errore: non è stato possibile determinare la data di conservazione dell'elemento: non è stato possibile determinare la data di conservazione dell'elemento

journal.cli.maintenance.health.prune.unreadable = { $indent }errore: { $detail }: { $detail }

journal.cli.maintenance.health.unavailable = i servizi di manutenzione dello stato non sono disponibili.

journal.cli.maintenance.health.usage = uso: { $command } { $id } { $syntax }{ $line_break }{ $command } { $id }: errore: argomenti non riconosciuti: { $args }

journal.cli.maintenance.help.list = uso: { $syntax }

journal.cli.maintenance.help.root = uso: { $syntax }

journal.cli.maintenance.help.run = uso: { $syntax }

journal.cli.maintenance.help.sync = uso: { $syntax }

journal.cli.maintenance.list.empty = Nessuna routine di manutenzione trovata.

journal.cli.maintenance.list.heading.description = DESCRIZIONE

journal.cli.maintenance.list.heading.every = OGNI

journal.cli.maintenance.list.heading.id = ID

journal.cli.maintenance.list.heading.runtime = DURATA MASSIMA

journal.cli.maintenance.list.heading.status = STATO

journal.cli.maintenance.list.status.divergent = divergente

journal.cli.maintenance.list.status.missing = mancante

journal.cli.maintenance.list.status.nothere = n/a

journal.cli.maintenance.list.status.synced = sincronizzato

journal.cli.maintenance.missing.root = uso: { $syntax }{ $line_break }{ $command }: errore: i seguenti argomenti sono obbligatori: { $argument }

journal.cli.maintenance.missing.run = uso: { $syntax }{ $line_break }{ $command }: errore: i seguenti argomenti sono obbligatori: { $argument }

journal.cli.maintenance.retention.import.owed = pulizia dell'audio importato incompleta: { $error }

journal.cli.maintenance.retention.owed = pulizia degli originali della tua voce incompleta: { $error }

journal.cli.maintenance.retention.summary = { $routine }: eliminati { $deleted }, eliminati senza un registro { $deleted_unrecorded }, già assenti { $gone }, ancora dovuti { $owed }, tenuti per sempre { $refused }, non provabili { $not_provable }, niente dovuto { $not_owed }, saltati mentre era occupato { $skipped }, eventi illeggibili { $unknown }

journal.cli.maintenance.retention.usage = uso: { $id } non accetta argomenti.

journal.cli.maintenance.routine.unimplemented = { $count_quantity ->
    [one] la routine di manutenzione { $id } non è ancora implementata ({ $count } argomento inoltrato).
    [many] la routine di manutenzione { $id } non è ancora implementata ({ $count } di argomenti inoltrati).
   *[other] la routine di manutenzione { $id } non è ancora implementata ({ $count } argomenti inoltrati).
}

journal.cli.maintenance.routine.unknown = Routine di manutenzione sconosciuta: { $id }. Esegui `{ $command }` per vedere le routine disponibili.

journal.cli.maintenance.schedule.error = Errore di lettura/aggiornamento di { $path }: { $error } (causa: { $error })

journal.cli.maintenance.schedule.io = l'I/O della pianificazione non è riuscito: { $detail }

journal.cli.maintenance.schedule.malformed = configurazione delle pianificazioni malformata in { $path }

journal.cli.maintenance.schedule.metadata = chiavi di metadati della pianificazione sconosciute: { $keys }

journal.cli.maintenance.schedule.shape = { $path }: la configurazione delle pianificazioni deve essere un oggetto JSON

journal.cli.maintenance.schedule.state.shape = lo stato della pianificazione in { $path } deve essere un oggetto JSON

journal.cli.maintenance.speakers.discovery.helper = l'ausilio di scoperta dei parlanti non è riuscito in { $stage }: { $reason }

journal.cli.maintenance.speakers.discovery.input = scoperta dei parlanti non riuscita: { $detail }

journal.cli.maintenance.speakers.error.owner.voice.look.failed = ricerca della tua voce non riuscita: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.consolidation.failed = consolidamento dei candidati a parlante non riuscito: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.pair.refresh.failed = aggiornamento delle coppie di candidati a parlante non riuscito: { $error }

journal.cli.maintenance.speakers.error.speaker.label.backfill.failed = riempimento delle etichette dei parlanti non riuscito: { $error }

journal.cli.maintenance.speakers.error.speaker.name.variant.refresh.failed = aggiornamento delle varianti di nome dei parlanti non riuscito: { $error }

journal.cli.maintenance.speakers.not.admitted = la generazione della scoperta dei parlanti non è stata ammessa all'ingresso della manutenzione

journal.cli.maintenance.speakers.usage.backfill.labels = uso: { $command }

journal.cli.maintenance.speakers.usage.backfill.pool = uso: { $command }

journal.cli.maintenance.speakers.usage.candidate.pair.suggestions = uso: { $command }

journal.cli.maintenance.speakers.usage.consolidate.pool = uso: { $command }

journal.cli.maintenance.speakers.usage.discover.voices = uso: { $command }

journal.cli.maintenance.speakers.usage.name.variants = uso: { $command }

journal.cli.maintenance.sync.added = aggiunti: { $count }: { $ids }

journal.cli.maintenance.sync.added.empty = aggiunti: 0

journal.cli.maintenance.sync.disabled = disattivati: { $count }: { $ids }

journal.cli.maintenance.sync.disabled.empty = disattivati: 0

journal.cli.maintenance.sync.divergent = divergenti: { $count }: { $ids }

journal.cli.maintenance.sync.divergent.empty = divergenti: 0

journal.cli.maintenance.sync.removed = rimossi: { $count }: { $ids }

journal.cli.maintenance.sync.removed.empty = rimossi: 0

journal.cli.maintenance.sync.synced = sincronizzati: { $count }: { $ids }

journal.cli.maintenance.sync.synced.empty = sincronizzati: 0

journal.cli.maintenance.sync.warning.disabled = AVVISO: la pianificazione { $id } è disattivata; è stata conservata disattivata

journal.cli.maintenance.sync.warning.divergent = AVVISO: la pianificazione { $id } è divergente; è stata conservata invariata

journal.cli.maintenance.trash.erased = cestino: cancellati { $erased }, finiti { $finished }, in attesa { $waiting }, non riusciti { $failed }

journal.cli.maintenance.trash.error = cestino: { $failure }

journal.cli.maintenance.trash.usage = uso: { $id } non accetta argomenti.

journal.cli.maintenance.usage.face = uso: { $syntax }{ $line_break }{ $command }: errore: argomenti non riconosciuti: argomenti imprevisti di riconciliazione dei volti

journal.cli.maintenance.usage.unknown = uso: { $syntax }{ $line_break }{ $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.offload.guidance.changed.retry = il trasferimento si è fermato perché il tuo diario è cambiato mentre era in esecuzione. non è stato rimosso nulla da questo computer. eseguilo di nuovo.

journal.cli.offload.guidance.changed.review = il trasferimento si è fermato perché il tuo diario è cambiato mentre era in esecuzione. non è stato rimosso nulla da questo computer. tieni i file di recupero del trasferimento e rivedi gli effetti annotati prima di riprovare.

journal.cli.offload.guidance.clock = non è stato possibile confermare l'orologio dell'operazione.

journal.cli.offload.guidance.journal = non è stato possibile confermare il tuo diario. tieni i suoi file di recupero per la revisione.

journal.cli.offload.guidance.platform = l'esecuzione del trasferimento non è disponibile su questa piattaforma.

journal.cli.offload.guidance.start = il trasferimento non è partito.

journal.cli.offload.guidance.tool = usa il comando { $solstone } installato con il suo strumento di backup incluso.

journal.cli.offload.guidance.unfinished = il trasferimento non è finito. tieni i suoi file di recupero e rivedi gli effetti annotati prima di riprovare.

journal.cli.offload.result.archive.failed = il passaggio di archiviazione del trasferimento non si è completato

journal.cli.offload.result.backup.failing = il tuo ultimo backup non si è completato correttamente

journal.cli.offload.result.backup.not.ready = il tuo backup non è configurato o non ha ancora completato un'esecuzione riuscita

journal.cli.offload.result.budget.already.satisfied = il budget del trasferimento era già stato raggiunto, quindi non serviva segnare nulla per il rilascio

journal.cli.offload.result.budget.satisfied = è stato segnato abbastanza media per il rilascio da raggiungere il budget del trasferimento

journal.cli.offload.result.confirm.failed = non è stato possibile confermare che i file archiviati siano arrivati davvero al backup

journal.cli.offload.result.confirm.tool.failed = non è stato possibile completare il controllo che conferma un'archiviazione del trasferimento

journal.cli.offload.result.markable.media.exhausted = ogni file idoneo era già stato controllato; non si può segnare altro per il rilascio ora

journal.cli.offload.result.other = fermato senza un motivo ({ $reason })

journal.cli.offload.result.unexpected.error = qualcosa di imprevisto ha interrotto il trasferimento

journal.cli.offload.result.verification.failed = l'ultima verifica del backup ha trovato un problema di integrità del backup

journal.cli.offload.result.verification.missing = nessuna verifica del backup è stata ancora completata

journal.cli.offload.result.verification.overdue = la verifica del backup non ha avuto successo abbastanza di recente

journal.cli.offload.stopped.archive.cancelled = trasferimento fermato: l'operazione di archiviazione è stata annullata.

journal.cli.offload.stopped.archive.clock.changed = trasferimento fermato: l'orologio di archiviazione è andato indietro.

journal.cli.offload.stopped.archive.configuration.changed = trasferimento fermato: la configurazione di archiviazione è cambiata.

journal.cli.offload.stopped.archive.configuration.unavailable = trasferimento fermato: non è stato possibile leggere la configurazione di archiviazione.

journal.cli.offload.stopped.archive.credentials.expired = trasferimento fermato: le credenziali di archiviazione sono scadute.

journal.cli.offload.stopped.archive.deadline.exceeded = trasferimento fermato: è stata raggiunta la scadenza dell'operazione di archiviazione.

journal.cli.offload.stopped.archive.invalid.credential.expiry = trasferimento fermato: le credenziali di archiviazione hanno una scadenza non valida.

journal.cli.offload.stopped.archive.invalid.selection = trasferimento fermato: le origini di archiviazione devono essere una selezione non vuota di percorsi assoluti.

journal.cli.offload.stopped.archive.journal.identity.changed = trasferimento fermato: non è stato possibile confermare il tuo diario.

journal.cli.offload.stopped.archive.not.configured = trasferimento fermato: il backup di archiviazione non è configurato.

journal.cli.offload.stopped.archive.repository.identity.changed = trasferimento fermato: il backup non corrisponde più.

journal.cli.offload.stopped.archive.repository.identity.invalid = trasferimento fermato: non è stato possibile identificare il backup.

journal.cli.offload.stopped.archive.snapshot.identity.invalid = trasferimento fermato: l'archivio non ha restituito un'identità completa dell'istantanea.

journal.cli.offload.stopped.archive.source.unavailable = trasferimento fermato: non è stato possibile confermare i file di archiviazione preparati.

journal.cli.offload.stopped.archive.tool.unavailable = trasferimento fermato: uno strumento di archiviazione non è disponibile.

journal.cli.offload.stopped.creation = trasferimento fermato: la creazione delle proposte non è finita.

journal.cli.offload.stopped.inputs.io = trasferimento fermato: non è stato possibile aprire o leggere un file nel tuo diario.

journal.cli.offload.stopped.inputs.space = trasferimento fermato: spazio libero insufficiente su questo computer ({ $available } liberi, { $required } necessari).

journal.cli.offload.stopped.native.start = trasferimento fermato: avvio dell'archiviazione rifiutato (stato { $status }).

journal.cli.offload.stopped.plan.incomplete = trasferimento fermato: la preparazione delle proposte non è finita.

journal.cli.offload.stopped.planning = trasferimento fermato: non è stato possibile preparare le proposte del trasferimento.

journal.cli.offload.stopped.readback = trasferimento fermato: la lettura di ritorno dal tuo backup non è riuscita.

journal.cli.offload.stopped.readback.credentials.unavailable = trasferimento fermato: le credenziali di lettura dell'archiviazione non sono disponibili.

journal.cli.offload.stopped.readback.incomplete.discovery = trasferimento fermato: la scansione dell'archiviazione non ha tenuto conto di ogni nodo.

journal.cli.offload.stopped.readback.invalid.selection = trasferimento fermato: il file di archiviazione selezionato non è valido.

journal.cli.offload.stopped.readback.native.failure = trasferimento fermato: non è stato possibile preparare il file archiviato.

journal.cli.offload.stopped.readback.not.configured = trasferimento fermato: la lettura dei file archiviati non è configurata.

journal.cli.offload.stopped.readback.snapshot.node.unavailable = trasferimento fermato: non è stato possibile confermare il file archiviato.

journal.cli.offload.stopped.readback.target.root.changed = trasferimento fermato: non è stato possibile preparare il file nel tuo diario.

journal.cli.offload.stopped.readback.tool.changed = trasferimento fermato: lo strumento di archiviazione installato è cambiato; riprova.

journal.cli.offload.stopped.readback.tool.failed = trasferimento fermato: il comando di lettura dell'archiviazione non è riuscito.

journal.cli.offload.stopped.readback.tool.unavailable = trasferimento fermato: lo strumento di archiviazione installato non è disponibile.

journal.cli.offload.stopped.readback.unsupported = trasferimento fermato: la lettura dei file archiviati non è disponibile su questa piattaforma.

journal.cli.offload.stopped.readback.unsupported.backend = trasferimento fermato: questa destinazione di backup non supporta la lettura dei file archiviati.

journal.cli.offload.stopped.readback.unsupported.tool.version = trasferimento fermato: la versione dello strumento di archiviazione installato non è supportata.

journal.cli.offload.stopped.record = trasferimento fermato: il salvataggio del registro del trasferimento non è riuscito.

journal.cli.offload.stopped.selection = trasferimento fermato: non è stato possibile confermare le registrazioni selezionate.

journal.cli.offload.stopped.temporary.copy = trasferimento fermato: la creazione di una copia temporanea non è riuscita.

journal.cli.offload.stopped.tool.reason = trasferimento fermato: { $reason }.

journal.cli.offload.stopped.unfinished.work = trasferimento fermato: il tuo diario ha ancora del lavoro non finito.

journal.cli.reprocess.empty.range = nessun dato per i giorni da { $day } a { $through }

journal.cli.reprocess.facet.classification = { $path }: classificazione della facetta non valida

journal.cli.reprocess.facet.utf8 = non-{ $encoding } facetta

journal.cli.reprocess.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $line_break }Invia un giorno passato del diario per la rielaborazione{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $day_column }Giorno passato nel formato { $date_format }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_9 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $through_option }{ $gap_2 }Fine inclusiva dell'intervallo nel formato { $date_format }{ $line_break }{ $syntax_continuation_13 }{ $line_break }{ $indent_2 }{ $unit_option }{ $gap_8 }Richiedi di riprovare un passaggio giornaliero di elaborazione non riuscito{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_6 }Faccetta di quell'unità; obbligatoria quando l'unità ha una faccetta{ $line_break }{ $indent_2 }{ $from_scratch_option }{ $gap_5 }Forza una riesecuzione giornaliera completa, preservando i marcatori (non{ $line_break }                     contrassegna il giorno come aggiornato){ $line_break }{ $indent_2 }{ $mark_updated_option }{ $gap_5 }Contrassegna il giorno come avente nuovi dati grezzi così l'elaborazione giornaliera{ $line_break }                     lo rimette in coda; sollecita poi uno svuotamento{ $line_break }{ $indent_2 }{ $owed_option }{ $gap_13 }Elenca gli output giornalieri dovuti nel giorno o nell'intervallo e perché,{ $line_break }                     senza inviare nulla{ $line_break }{ $indent_2 }{ $json_option }{ $gap_13 }Con { $owed_flag }, riporta ogni giorno come stati e conteggi: i suoi{ $line_break }                     marcatori, la sua copertura, quanti output giornalieri ha e{ $line_break }                     quanti sono dovuti, per causa{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_6 }Abilita l'output dettagliato{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_8 }Abilita il logging di debug

journal.cli.reprocess.input.day = giorno non valido: { $day }

journal.cli.reprocess.intent.busy = il giorno richiesto è in carico a un altro esecutore

journal.cli.reprocess.intent.invalid.day = giorno di lavoro non valido

journal.cli.reprocess.intent.uncertain = la pubblicazione dell'intento di lavoro è incerta: { $outcome }

journal.cli.reprocess.intent.unrecognized = intento di lavoro non riconosciuto

journal.cli.reprocess.native.arguments = nativo { $binary } { $verb } non riuscito: gli argomenti non sono { $encoding } validi

journal.cli.reprocess.no.data = nessun dato per il giorno { $day }

journal.cli.reprocess.no.engine = non è ancora stato scelto un modello. scegline uno in ragionamento, poi riprova

journal.cli.reprocess.owed.internal = { $flag } riporta soltanto; usa { $function }

journal.cli.reprocess.owed.only = { $owed_flag } riporta soltanto; non è stato inviato nulla

journal.cli.reprocess.past.only = la rielaborazione è solo per il passato (non è possibile rielaborare oggi o un giorno futuro)

journal.cli.reprocess.path.utf8 = percorso della fonte non-{ $encoding }

journal.cli.reprocess.range.plan = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] piano di rielaborazione da zero:{ $break }{ $days } giorno con dati ({ $segments } segmento) verrà messo in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questo giorno può richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
        [many] piano di rielaborazione da zero:{ $break }{ $days } giorno con dati ({ $segments } di segmenti) verrà messo in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questo giorno può richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
       *[other] piano di rielaborazione da zero:{ $break }{ $days } giorno con dati ({ $segments } segmenti) verrà messo in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questo giorno può richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
    }
    [many] { $segments_quantity ->
        [one] piano di rielaborazione da zero:{ $break }{ $days } di giorni con dati ({ $segments } segmento) verranno messi in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questi giorni possono richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
        [many] piano di rielaborazione da zero:{ $break }{ $days } di giorni con dati ({ $segments } di segmenti) verranno messi in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questi giorni possono richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
       *[other] piano di rielaborazione da zero:{ $break }{ $days } di giorni con dati ({ $segments } segmenti) verranno messi in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questi giorni possono richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
    }
   *[other] { $segments_quantity ->
        [one] piano di rielaborazione da zero:{ $break }{ $days } giorni con dati ({ $segments } segmento) verranno messi in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questi giorni possono richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
        [many] piano di rielaborazione da zero:{ $break }{ $days } giorni con dati ({ $segments } di segmenti) verranno messi in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questi giorni possono richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
       *[other] piano di rielaborazione da zero:{ $break }{ $days } giorni con dati ({ $segments } segmenti) verranno messi in coda. L'avanzamento sarà visibile in { $top } o { $health }. Le richieste sopravvivono a un riavvio del supervisor; un giorno che non può finire da zero finisce come rielaborazione ordinaria.{ $break }Questi giorni possono richiedere ore; il lavoro di oggi ha la precedenza tra un compito e l'altro.{ $break }riesegui con { $yes } per procedere
    }
}

journal.cli.reprocess.range.saved = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] richieste da zero salvate per { $days } giorno ({ $segments } segmento){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
        [many] richieste da zero salvate per { $days } giorno ({ $segments } di segmenti){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
       *[other] richieste da zero salvate per { $days } giorno ({ $segments } segmenti){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
    }
    [many] { $segments_quantity ->
        [one] richieste da zero salvate per { $days } di giorni ({ $segments } segmento){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
        [many] richieste da zero salvate per { $days } di giorni ({ $segments } di segmenti){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
       *[other] richieste da zero salvate per { $days } di giorni ({ $segments } segmenti){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
    }
   *[other] { $segments_quantity ->
        [one] richieste da zero salvate per { $days } giorni ({ $segments } segmento){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
        [many] richieste da zero salvate per { $days } giorni ({ $segments } di segmenti){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
       *[other] richieste da zero salvate per { $days } giorni ({ $segments } segmenti){ $break }l'avanzamento è visibile in health; il lavoro di oggi ha la precedenza
    }
}

journal.cli.reprocess.record.invalid = Record dell'unità giornaliera malformato: identità dell'unità giornaliera non valida

journal.cli.reprocess.record.io = Errore di I/O: { $cause }

journal.cli.reprocess.record.mismatch = Record dell'unità giornaliera malformato: identità non corrispondente nel record in { $path }: atteso { $expected }, trovato { $found }

journal.cli.reprocess.retry.accepted = questo output giornaliero è già accettato

journal.cli.reprocess.retry.disabled = il talento giornaliero non è più abilitato

journal.cli.reprocess.retry.invalid.day = giorno del nuovo tentativo non valido

journal.cli.reprocess.retry.invalid.unit = unità del nuovo tentativo non valida

journal.cli.reprocess.retry.missing = manca l'unità giornaliera

journal.cli.reprocess.retry.missing.evidence = manca la prova trattenuta

journal.cli.reprocess.retry.no.longer.held = l'unità non è più trattenuta

journal.cli.reprocess.retry.not.daily = non è lavoro giornaliero

journal.cli.reprocess.retry.state = questa unità giornaliera non può essere riprovata senza risolverne lo stato attuale

journal.cli.reprocess.schedule.lookback = rilettura della pianificazione giornaliera fuori da { $range }

journal.cli.reprocess.schema.missing = manca il percorso del prompt dello schema

journal.cli.reprocess.schema.name = lo schema deve essere un nome di file

journal.cli.reprocess.shape.invalid = forma non valida per { $path }

journal.cli.reprocess.shape.object = il sidecar della forma non è un oggetto

journal.cli.reprocess.shape.written = forma della fonte scritta non valida: { $path }

journal.cli.reprocess.source.json.object = { $path } non è un oggetto { $encoding }

journal.cli.reprocess.source.line.error = { $path } riga { $line }: { $error }

journal.cli.reprocess.source.line.object = { $path } riga { $line } non è un oggetto

journal.cli.reprocess.source.symlink = la fonte giornaliera è un collegamento simbolico: { $path }

journal.cli.reprocess.source.utf8 = percorso della fonte giornaliera non-{ $encoding }

journal.cli.reprocess.talent.overrides = non è stato possibile leggere le sostituzioni del talento da { $path }

journal.cli.reprocess.template.utf8 = nome di template non-{ $encoding }

journal.cli.reprocess.through.order = { $through_flag } deve essere nel giorno di inizio o dopo

journal.cli.reprocess.through.required = { $through_flag } richiede { $from_scratch_flag }

journal.cli.reprocess.unit.failed = unità di rielaborazione non riuscita: { $error }

journal.cli.reprocess.unit.saved = richiesta di nuovo tentativo salvata per { $unit } il { $day }

journal.cli.reprocess.unreadable = { $day }  (illeggibile: { $error })

journal.cli.reprocess.usage.conflict = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: errore: argomento { $option }: non consentito con l'argomento { $previous }

journal.cli.reprocess.usage.missing.day = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: errore: i seguenti argomenti sono obbligatori: { $positional }

journal.cli.reprocess.usage.missing.value = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: errore: argomento { $option }: è atteso un argomento

journal.cli.reprocess.usage.requires = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: errore: argomento { $option }: richiede { $required }

journal.cli.reprocess.usage.unknown = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.reprocess.waiting = richiesta salvata; avvia il servizio in background con { $start_command } per elaborarla

journal.cli.retention.proposal.eligible = idoneità della politica: Eligible { $open_brace } anchor: { $anchor }, age_days: { $age_days }, period: Days({ $period_days }) { $close_brace }

journal.cli.retention.proposal.forever = idoneità della politica: KeptForever

journal.cli.retention.proposal.no.anchor = idoneità della politica: AnchorMissing { $open_brace } anchor: { $anchor } { $close_brace }

journal.cli.retention.proposal.unrecorded = rimozione non riuscita prima che una proposta fosse scritta

journal.cli.retention.proposal.young = idoneità della politica: TooYoung { $open_brace } anchor: { $anchor }, age_days: { $age_days }, period: Days({ $period_days }) { $close_brace }

journal.cli.retention.recovery.activity.binding = non è stato possibile controllare il tuo diario, quindi questa eliminazione si è fermata

journal.cli.retention.recovery.activity.busy = il tuo diario è occupato con un altro lavoro

journal.cli.retention.recovery.activity.hold = un'altra operazione del diario richiede attenzione prima che questa rimozione possa continuare

journal.cli.retention.recovery.activity.reserved = questa posizione è riservata a un'operazione del diario e non può essere rimossa

journal.cli.retention.recovery.path.directory = c'è una cartella dove era attesa una registrazione

journal.cli.retention.recovery.path.invalid = la posizione non nomina un file dentro il tuo diario

journal.cli.retention.recovery.path.outside = questo non è dentro il tuo diario, quindi è stato lasciato stare

journal.cli.retention.recovery.path.permission = non è stato possibile leggere o rimuovere il file, il che di solito indica un problema di permessi

journal.cli.retention.recovery.path.unavailable = non è stato possibile rimuovere il file; il diario è stato lasciato invariato

journal.cli.retention.recovery.record.prepare = non è stato possibile preparare il record di questa rimozione

journal.cli.retention.recovery.record.write = non è stato possibile scrivere il record di questa rimozione

journal.cli.retention.recovery.segment.busy = un altro processo sta lavorando su questo segmento

journal.cli.retention.recovery.segment.gone = questo segmento è già stato rimosso

journal.cli.retention.recovery.segment.live.occupied = è stato scritto qualcosa di nuovo dove c'era questo segmento, quindi è stato messo da parte invece di essere sovrascritto

journal.cli.retention.recovery.segment.missing = non c'è questo segmento nel tuo diario

journal.cli.retention.recovery.segment.staged = questo segmento è a metà della rimozione ed è messo da parte; non è elencato con gli altri tuoi segmenti finché non finisce

journal.cli.retention.recovery.segment.unconfirmed = non è stato possibile confermare che sia sparito, quindi non viene segnalato come rimosso

journal.cli.retention.recovery.segment.unfinished = una rimozione precedente di questo segmento non è finita; va guardata prima che questa possa essere riprovata

journal.cli.retention.removal.uncertain = non è stato possibile completare o confermare l'eliminazione. serve il ripristino.

journal.cli.schedule.cell.disabled = disattivato

journal.cli.schedule.cell.invalid = non valido

journal.cli.schedule.cell.midnight = mezzanotte

journal.cli.schedule.cell.never = mai

journal.cli.schedule.cell.now = adesso

journal.cli.schedule.config = Configurazione: { $path }

journal.cli.schedule.empty = Nessuna pianificazione configurata.{ $line_break }{ $line_break }Aggiungi pianificazioni in: { $path }

journal.cli.schedule.error.cmd.invalid = la pianificazione '{ $name }' ha un cmd non valido

journal.cli.schedule.error.config.malformed = la configurazione delle pianificazioni in { $path } è malformata

journal.cli.schedule.error.config.not.object = la configurazione delle pianificazioni in { $path } deve essere un oggetto JSON

journal.cli.schedule.error.config.unreadable = la configurazione delle pianificazioni in { $path } è illeggibile

journal.cli.schedule.error.entry.invalid = la pianificazione '{ $name }' non è un elemento valido

journal.cli.schedule.error.state.malformed = lo stato dello scheduler in { $path } è malformato

journal.cli.schedule.error.state.not.object = lo stato dello scheduler in { $path } deve essere un oggetto JSON

journal.cli.schedule.error.state.unreadable = lo stato dello scheduler in { $path } è illeggibile

journal.cli.schedule.header.cmd = COMANDO

journal.cli.schedule.header.every = OGNI

journal.cli.schedule.header.last.run = ULTIMA ESECUZIONE

journal.cli.schedule.header.name = NOME

journal.cli.schedule.header.next.due = PROSSIMA SCADENZA

journal.cli.schedule.tag.disabled = [disattivato]

journal.cli.schedule.weekly.friday = Venerdì { $time }

journal.cli.schedule.weekly.monday = Lunedì { $time }

journal.cli.schedule.weekly.saturday = Sabato { $time }

journal.cli.schedule.weekly.sunday = Domenica { $time }

journal.cli.schedule.weekly.thursday = Giovedì { $time }

journal.cli.schedule.weekly.tuesday = Martedì { $time }

journal.cli.schedule.weekly.wednesday = Mercoledì { $time }

journal.cli.segment.argument.missing = uso: { $syntax }{ $line_break }{ $command }: errore: i seguenti argomenti sono obbligatori: { $input }

journal.cli.segment.argument.unknown = uso: { $syntax }{ $line_break }{ $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.segment.check.backward = catena all'indietro

journal.cli.segment.check.chain.no.day = { $prev_segment_key } impostato senza { $prev_day_key }

journal.cli.segment.check.chain.no.successor = segmento successivo non trovato, non coda del flusso

journal.cli.segment.check.chain.previous = segmento precedente trovato

journal.cli.segment.check.chain.start = nessun segmento precedente

journal.cli.segment.check.chain.tail = coda del flusso

journal.cli.segment.check.content.empty = né { $audio_file }, né { $screen_file }, né { $browser_files }

journal.cli.segment.check.content.present = file di contenuto presenti

journal.cli.segment.check.directory.absent = directory del segmento mancante

journal.cli.segment.check.directory.exists = directory esiste

journal.cli.segment.check.directory.missing = directory mancante

journal.cli.segment.check.forward = catena in avanti

journal.cli.segment.check.index = presenza nell'indice

journal.cli.segment.check.index.absent = indice del diario non disponibile

journal.cli.segment.check.index.error = errore dell'indice del diario: { $error } (esegui: { $index_command })

journal.cli.segment.check.index.missing = segmento non indicizzato

journal.cli.segment.check.index.present = segmento indicizzato

journal.cli.segment.check.marker.exists = { $stream_file } esiste

journal.cli.segment.check.marker.invalid = { $stream_file } non valido { $json_format }

journal.cli.segment.check.marker.missing = { $stream_file } mancante

journal.cli.segment.check.marker.no.stream = { $stream_file } campo flusso mancante

journal.cli.segment.check.marker.valid = { $stream_file } valido

journal.cli.segment.check.next = segmento successivo { $path }

journal.cli.segment.check.previous.missing = segmento precedente mancante { $path }

journal.cli.segment.day.empty = Nessun segmento trovato per { $day }

journal.cli.segment.guard.activity = un'altra operazione del diario richiede attenzione prima che questa modifica possa continuare

journal.cli.segment.guard.binding = non è stato possibile controllare il diario, quindi questa modifica è in sospeso

journal.cli.segment.guard.reserved = questa posizione è riservata a un'operazione del diario e non può essere modificata

journal.cli.segment.help.inspect = uso: { $syntax }{ $break }{ $break }argomenti posizionali:{ $break }{ $option_3 }Percorso del segmento: { $example_path } (es.{ $break }{ $syntax_line_4 }{ $break }{ $break }opzioni:{ $break }{ $option_7 }mostra questo messaggio di aiuto ed esci{ $break }{ $option_8 }Output come { $json_format }

journal.cli.segment.help.list = uso: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $break }argomenti posizionali:{ $break }{ $option_5 }Giorno nel formato { $day_format }{ $break }{ $break }opzioni:{ $break }{ $option_8 }mostra questo messaggio di aiuto ed esci{ $break }{ $option_9 }Filtra su un flusso specifico{ $break }{ $option_10 }Output come { $json_format }

journal.cli.segment.help.move = uso: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $syntax_line_3 }{ $break }{ $break }argomenti posizionali:{ $break }{ $option_6 }Percorso del segmento: { $example_path } (es.{ $break }{ $syntax_line_7 }{ $break }{ $break }opzioni:{ $break }{ $option_10 }mostra questo messaggio di aiuto ed esci{ $break }{ $option_11 }Giorno di destinazione ({ $day_format }){ $break }{ $option_12 }Nuova ora ({ $time_format }), mantenendo la durata originale{ $break }{ $option_13 }Mostra il piano senza fare modifiche{ $break }{ $option_14 }Mostra l'avanzamento dettagliato

journal.cli.segment.help.top = uso: { $syntax }{ $break }{ $break }Ispeziona e gestisci i segmenti del diario{ $break }{ $break }argomenti posizionali:{ $break }{ $syntax_line_5 }{ $break }{ $option_6 }Elenca i segmenti di un giorno{ $break }{ $option_7 }Mostra i metadati del segmento{ $break }{ $option_8 }Verifica l'integrità del segmento{ $break }{ $option_9 }Sposta il segmento a un altro giorno/ora{ $break }{ $break }opzioni:{ $break }{ $option_12 }mostra questo messaggio di aiuto ed esci{ $break }{ $option_13 }Abilita l'output dettagliato{ $break }{ $option_14 }Abilita il logging di debug

journal.cli.segment.help.verify = uso: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $break }argomenti posizionali:{ $break }{ $option_4 }Percorso del segmento: { $example_path }{ $break }{ $break }opzioni:{ $break }{ $option_7 }mostra questo messaggio di aiuto ed esci{ $break }{ $option_8 }Verifica tutti i segmenti di un giorno{ $break }{ $option_9 }Output come { $json_format }

journal.cli.segment.identity.default = elenco segmenti rifiutato: la directory del flusso denominata "{ $default_stream }" non può essere scritta come identità di record: { $path }

journal.cli.segment.identity.duplicate = elenco segmenti rifiutato: più segmenti condividono il flusso { $stream } e la chiave { $key }

journal.cli.segment.identity.encoding = elenco segmenti rifiutato: il percorso del segmento non è rappresentabile in { $encoding }: { $path }

journal.cli.segment.identity.refused = elenco segmenti rifiutato: { $detail }

journal.cli.segment.inspect.chain = Catena:

journal.cli.segment.inspect.events = { $quantity ->
    [one] Eventi: { $entries } elemento
    [many] Eventi: { $entries } di elementi
   *[other] Eventi: { $entries } elementi
}

journal.cli.segment.inspect.events.tracts = { $quantity ->
    [one] Eventi: { $entries } elemento ({ $tracts })
    [many] Eventi: { $entries } di elementi ({ $tracts })
   *[other] Eventi: { $entries } elementi ({ $tracts })
}

journal.cli.segment.inspect.files = File ({ $count }):

journal.cli.segment.inspect.index.error = Indice: errore ({ $error }) - esegui: { $index_command }

journal.cli.segment.inspect.index.missing = Indice: non-indicizzato

journal.cli.segment.inspect.index.unavailable = Indice: non disponibile

journal.cli.segment.inspect.indexed = { $quantity ->
    [one] Indice: indicizzato ({ $chunks } blocco)
    [many] Indice: indicizzato ({ $chunks } di blocchi)
   *[other] Indice: indicizzato ({ $chunks } blocchi)
}

journal.cli.segment.inspect.next = successivo: { $next }

journal.cli.segment.inspect.next.none = successivo: (nessuno)

journal.cli.segment.inspect.next.tail = successivo: (coda)

journal.cli.segment.inspect.path = Segmento: { $path }

journal.cli.segment.inspect.prev = precedente: { $previous }

journal.cli.segment.inspect.prev.missing = precedente: { $path } [MANCANTE]

journal.cli.segment.inspect.prev.none = precedente: (nessuno)

journal.cli.segment.inspect.size = Dimensione: { $size }

journal.cli.segment.inspect.stream = Flusso:  { $stream }

journal.cli.segment.inspect.stream.seq = Flusso:  { $stream } (seq { $sequence })

journal.cli.segment.inspect.talents = Talenti ({ $count }):

journal.cli.segment.inspect.time = Ora:    { $range } ({ $seconds }s)

journal.cli.segment.list.heading.dur = DURATA

journal.cli.segment.list.heading.files = FILE

journal.cli.segment.list.heading.segment = SEGMENTO

journal.cli.segment.list.heading.size = DIMENSIONE

journal.cli.segment.list.heading.stream = FLUSSO

journal.cli.segment.list.heading.talents = TALENTI

journal.cli.segment.list.heading.time = ORA

journal.cli.segment.missing = Segmento non trovato: { $path }

journal.cli.segment.move.day.format = Formato { $to_day_flag } non valido: { $to_day } (atteso { $date_format })

journal.cli.segment.move.directory.created = directory creata: { $path }

journal.cli.segment.move.directory.exists = Destinazione { $destination } esiste già; nessuna modifica fatta

journal.cli.segment.move.directory.failed = spostamento della directory al passo 1 non riuscito: { $error }; origine e indice restano autorevoli

journal.cli.segment.move.directory.moved = directory spostata: { $source } -> { $destination }

journal.cli.segment.move.dry = [prova a secco] Nessuna modifica fatta

journal.cli.segment.move.events.none = nessun { $event_file } da riscrivere

journal.cli.segment.move.events.rewritten = { $quantity ->
    [one] riscritta { $count } riga { $event_file } (giorno: { $source_day }->{ $destination_day }, segmento: { $source }->{ $destination })
    [many] riscritte { $count } di righe { $event_file } (giorno: { $source_day }->{ $destination_day }, segmento: { $source }->{ $destination })
   *[other] riscritte { $count } righe { $event_file } (giorno: { $source_day }->{ $destination_day }, segmento: { $source }->{ $destination })
}

journal.cli.segment.move.exists = Il segmento { $segment } esiste già in { $day }/{ $stream }. Usa { $to_time_flag } per specificare un'ora alternativa.

journal.cli.segment.move.fail.ambiguous = { $quantity ->
    [one] patch del successore al passo 3 incoerente dopo lo spostamento: catena ambigua: { $count } successore lasciato non patchato
    [many] patch del successore al passo 3 incoerente dopo lo spostamento: catena ambigua: { $count } di successori lasciati non patchati
   *[other] patch del successore al passo 3 incoerente dopo lo spostamento: catena ambigua: { $count } successori lasciati non patchati
}

journal.cli.segment.move.fail.events = riscrittura degli eventi al passo 2 incoerente dopo lo spostamento: { $detail }

journal.cli.segment.move.fail.health.marker = tocco del marcatore di stato al passo 6 incoerente dopo lo spostamento: { $detail }

journal.cli.segment.move.fail.search = aggiornamento della ricerca al passo 5 incoerente dopo lo spostamento: non è stato possibile mettere in coda l'aggiornamento della ricerca: { $error }

journal.cli.segment.move.fail.successor = patch del successore al passo 3 incoerente dopo lo spostamento: { $detail }

journal.cli.segment.move.fail.tail.locked = riparazione della coda del flusso al passo 4 incoerente dopo lo spostamento: non è stato possibile bloccare il record del flusso

journal.cli.segment.move.fail.tail.malformed = riparazione della coda del flusso al passo 4 incoerente dopo lo spostamento: record del flusso malformato

journal.cli.segment.move.fail.tail.no.record = riparazione della coda del flusso al passo 4 incoerente dopo lo spostamento: nessun record del flusso; l'identità non è stata riscritta

journal.cli.segment.move.fail.tail.write = riparazione della coda del flusso al passo 4 incoerente dopo lo spostamento: pubblicazione atomica non riuscita

journal.cli.segment.move.markers.touched = marcatori di stato toccati: { $days }

journal.cli.segment.move.no.duration = La chiave del segmento di origine non ha durata

journal.cli.segment.move.no.marker = Nessun { $stream_file } nel segmento di origine

journal.cli.segment.move.no.slot = Nessuno slot di segmento disponibile vicino a { $destination_segment } il { $to_day }

journal.cli.segment.move.plan = Spostamento: { $source } -> { $destination }

journal.cli.segment.move.plan.ambiguous = { $quantity ->
    [one] successore da patchare: (ambiguo: { $count } successore)
    [many] successore da patchare: (ambiguo: { $count } di successori)
   *[other] successore da patchare: (ambiguo: { $count } successori)
}

journal.cli.segment.move.plan.events = { $event_file } righe: { $count }

journal.cli.segment.move.plan.index = blocchi dell'indice: { $chunks }

journal.cli.segment.move.plan.index.error = errore di lettura dell'indice: { $error } (si tenterà elimina+reindicizza; esegui: { $index_command })

journal.cli.segment.move.plan.markers = marcatori di stato: { $source_day }, { $destination_day }

journal.cli.segment.move.plan.successor = successore da patchare: { $path }

journal.cli.segment.move.plan.tail = successore da patchare: (nessuno - coda del flusso)

journal.cli.segment.move.running = Spostamento in corso...

journal.cli.segment.move.same = Origine e destinazione sono le stesse

journal.cli.segment.move.search.queued = aggiornamenti di ricerca in coda

journal.cli.segment.move.stream.mismatch = Flusso non corrispondente: il percorso dice '{ $path_stream }' ma { $stream_file } dice '{ $marker_stream }'

journal.cli.segment.move.stream.rebuilt = stato del flusso ricostruito: { $stream }

journal.cli.segment.move.successor.none = nessun successore da patchare (coda del flusso)

journal.cli.segment.move.successor.patched = successore patchato { $path }

journal.cli.segment.move.talents = { $command } rieseguirà i talenti giornalieri nei giorni toccati

journal.cli.segment.move.time.format = Formato { $to_time_flag } non valido: { $time } (atteso { $time_format })

journal.cli.segment.path.syntax = Il percorso del segmento deve essere { $path_form } (es. { $example })

journal.cli.segment.relocation.deleted = il segmento è stato eliminato dal suo titolare

journal.cli.segment.relocation.location = la posizione del segmento non corrisponde allo spostamento previsto

journal.cli.segment.relocation.marker = non è stato possibile leggere il marcatore

journal.cli.segment.status.pass = SUPERATO

journal.cli.segment.verify.day.result = { $quantity ->
    [one] Riepilogo: { $passed }/{ $total } controllo superato
    [many] Riepilogo: { $passed }/{ $total } di controlli superati
   *[other] Riepilogo: { $passed }/{ $total } controlli superati
}

journal.cli.segment.verify.input = { $verb } richiede un percorso di segmento o { $day_flag }

journal.cli.segment.verify.result = { $quantity ->
    [one] { $passed }/{ $total } controllo superato
    [many] { $passed }/{ $total } di controlli superati
   *[other] { $passed }/{ $total } controlli superati
}

journal.cli.share.activities = non è stato possibile leggere le attività { $facet } per { $day }: { $detail }

journal.cli.share.activity.missing = nessuna attività '{ $activity }' in { $facet } il { $day }

journal.cli.share.activity.unmatched = l'attività '{ $activity }' non corrisponde a { $person }

journal.cli.share.facets = non è stato possibile elencare le facette: { $detail }

journal.cli.share.failure = { $command }: { $detail }

journal.cli.share.help.match = uso: { $syntax }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }id entità, o un nome o alias univoco di una persona{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }il giorno da leggere{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }solo questa faccetta (predefinito: ogni faccetta)

journal.cli.share.help.package = uso: { $syntax }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }id entità, o un nome o alias univoco di una persona{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }il giorno dell'attività{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }la faccetta dell'attività{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_5 }l'id dell'attività

journal.cli.share.help.root = uso: { $syntax }{ $line_break }{ $line_break }Trova le attività condivise con una persona collegata e mostra l'anteprima di ciò che verrebbe condiviso.{ $line_break }Entrambi i sottocomandi leggono soltanto il diario.{ $line_break }{ $line_break }comandi:{ $line_break }{ $match_command }Elenca le attività del giorno che corrispondono a una persona, una riga { $json } ciascuna{ $line_break }{ $package_command }Stampa il pacchetto di riepilogo circoscritto per un'attività corrispondente

journal.cli.share.parse.argument = { $command }: errore: argomento non riconosciuto '{ $argument }'

journal.cli.share.parse.duplicate = { $command }: errore: { $flag } dato più di una volta

journal.cli.share.parse.encoding = { $command }: errore: gli argomenti devono essere { $encoding } validi

journal.cli.share.parse.missing = { $command }: errore: è obbligatorio un sottocomando

journal.cli.share.parse.required = { $command }: errore: { $option } è obbligatorio

journal.cli.share.parse.unknown = { $command }: errore: sottocomando sconosciuto '{ $argument }'

journal.cli.share.parse.value = { $command }: errore: { $flag } richiede un valore

journal.cli.share.provider.activity.unavailable = riepilogo dell'attività non disponibile: { $detail }

journal.cli.share.provider.child.cleanup = { $primary } (pulizia: { $cleanup })

journal.cli.share.provider.child.exit = uscita { $code }

journal.cli.share.provider.child.exit.signal = uscita { $code } segnale { $signal }

journal.cli.share.provider.child.invalid = risposta del processo figlio non valida ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process = I/O del processo non riuscito: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process.cleanup = I/O del processo non riuscito: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }; { $cleanup_field }{ $cleanup }

journal.cli.share.provider.child.protocol = protocollo { $reason } ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.signal = segnale { $signal }

journal.cli.share.provider.child.status.unknown = stato sconosciuto

journal.cli.share.provider.child.truncated = { $bytes } [troncato]

journal.cli.share.provider.child.unavailable = non disponibile

journal.cli.share.provider.child.unexpected = processo figlio inatteso ({ $status }); { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.entities.read = non è stato possibile leggere le entità: { $detail }

journal.cli.share.provider.entity.invalid = non è stato possibile caricare l'entità del diario { $entity }: { $detail }

journal.cli.share.provider.matching = corrispondenza { $facet }/{ $day }/{ $activity } per { $entity }

journal.cli.share.provider.model.unconfigured = Non è ancora stato scelto un modello. Scegline uno in Ragionamento.

journal.cli.share.provider.person.absent = nessuna persona in questo diario si chiama '{ $query }'

journal.cli.share.provider.person.ambiguous = '{ $query }' è il nome di più di una persona ({ $people }); usa un id di entità

journal.cli.share.provider.presence.unavailable = controllo della presenza non disponibile: { $detail }

journal.cli.share.provider.segment.unavailable = riepilogo del segmento non disponibile: { $detail }

journal.cli.share.provider.talent.invalid = talento '{ $talent }' ha restituito { $json } non valido: { $error }

journal.cli.share.provider.talent.prepare = talento '{ $talent }': { $detail }

journal.cli.share.provider.talent.timeout.numeric = la configurazione '{ $talent }' dichiara un { $timeout_s_field } non numerico

journal.cli.share.provider.talent.timeout.range = la configurazione '{ $talent }' dichiara { $timeout_s_field } fuori dall'intervallo supportato

journal.cli.share.provider.talent.tokens = talento '{ $talent }' non dichiara alcun { $max_output_tokens_field }

journal.cli.share.provider.talent.type = tipo di talento non supportato: { $kind }

journal.cli.share.provider.talent.unfinished = talento '{ $talent }' non ha terminato: { $outcome }

journal.cli.share.startup.failed = { $command } non riuscito: { $detail }

journal.cli.share.startup.runtime.current = non è stato possibile risolvere l'eseguibile del worker: { $error }

journal.cli.share.startup.runtime.parent = l'eseguibile del worker non ha un genitore: { $path }

journal.cli.share.startup.runtime.root = non è stato possibile risolvere la radice di installazione dalla directory dell'eseguibile: { $path }

journal.cli.share.startup.sibling.missing = { $command } non riuscito: manca l'eseguibile fratello { $path }

journal.cli.share.startup.sibling.parent = { $command } non riuscito: l'eseguibile corrente non ha un genitore

journal.cli.share.transport.child.inspect = ispezione del processo figlio prima della terminazione: { $detail }

journal.cli.share.transport.child.reap = reap del processo figlio: { $detail }

journal.cli.share.transport.child.retry = riprova del reap del processo figlio: { $detail }

journal.cli.share.transport.child.terminate = terminazione del processo figlio: { $detail }

journal.cli.share.transport.collector = { $pipe } collettore: { $detail }

journal.cli.share.transport.collector.panic = { $pipe } collettore: panic del thread

journal.cli.share.transport.field.array = { $field } deve essere un array o null

journal.cli.share.transport.field.boolean = { $field } deve essere un booleano

journal.cli.share.transport.field.optional = { $field } deve essere una stringa o null

journal.cli.share.transport.field.string = { $field } deve essere una stringa

journal.cli.share.transport.field.strings = { $field } deve contenere solo stringhe

journal.cli.share.transport.object = il record deve essere un oggetto { $json }

journal.cli.share.transport.outcome = l'esito della risposta non è supportato

journal.cli.share.transport.pipe.missing = il canale { $pipe } non è disponibile

journal.cli.share.transport.request.empty = { $contents } deve essere non vuoto

journal.cli.share.transport.request.overflow = { $overflow } richiede che la prima parte del contenuto sia testo

journal.cli.share.transport.schema = lo schema del record non è supportato

journal.cli.share.transport.stdout.limit = { $stdout } troncato a { $limit } byte

journal.cli.share.transport.usage = { $usage } è obbligatorio

journal.cli.share.transport.worker.channel = canale di completamento del worker di pipe: { $detail }

journal.cli.share.transport.worker.join = { $pipe } join del worker: panic del thread

journal.cli.share.transport.worker.start = avvio del worker { $pipe }: { $detail }

journal.cli.share.transport.writer = { $stdin } scrittore: { $detail }

journal.cli.share.transport.writer.panic = { $stdin } scrittore: panic del thread

journal.cli.share.usage.match = uso: { $syntax }

journal.cli.share.usage.package = uso: { $syntax }

journal.cli.share.usage.root = uso: { $syntax }

journal.cli.social.failed = non è stato possibile finire: { $detail }

journal.cli.social.follow.done = segui lo Space che { $sharer } ha condiviso con te.

journal.cli.social.receive.added = { $activity_quantity ->
    [one] { $sharer } ha condiviso { $activities } nuova attività con te.
    [many] { $sharer } ha condiviso { $activities } di nuove attività con te.
   *[other] { $sharer } ha condiviso { $activities } nuove attività con te.
}

journal.cli.social.receive.failed = non è stato possibile leggere lo Space che { $sharer } ha condiviso con te: { $detail }

journal.cli.social.receive.none = niente di nuovo da { $sharer }.

journal.cli.social.receive.nothing = non segui ancora nessuno Space.

journal.cli.social.receive.removed = { $activity_quantity ->
    [one] { $sharer } ha smesso di condividere { $activities } attività, quindi è sparita dal tuo diario.
    [many] { $sharer } ha smesso di condividere { $activities } di attività, quindi sono sparite dal tuo diario.
   *[other] { $sharer } ha smesso di condividere { $activities } attività, quindi sono sparite dal tuo diario.
}

journal.cli.social.receive.updated = { $activity_quantity ->
    [one] { $sharer } ha aggiornato { $activities } attività che ha condiviso con te.
    [many] { $sharer } ha aggiornato { $activities } di attività che ha condiviso con te.
   *[other] { $sharer } ha aggiornato { $activities } attività che ha condiviso con te.
}

journal.cli.social.resolve.done = { $account } è { $did }, con host { $host }.

journal.cli.social.share.done = condiviso "{ $title }". invia questo indirizzo dello Space alle persone con cui l'hai condiviso, così i loro diari possono seguirlo:{ $line_break }{ $space }

journal.cli.social.signed.out = non hai effettuato l'accesso. inizia con { $sign_in }

journal.cli.social.signin.declined = l'accesso è stato rifiutato, quindi non è cambiato nulla.

journal.cli.social.signin.done = accesso effettuato come { $account }.

journal.cli.social.signin.expired = l'accesso non è stato completato in tempo. eseguilo di nuovo per ricominciare.

journal.cli.social.signin.open = per accedere, apri questo indirizzo nel browser:{ $line_break }{ $address }

journal.cli.social.status.account = accesso effettuato come { $account } su { $host }.

journal.cli.social.status.memories = { $memory_quantity ->
    [one] tieni { $memories } attività condivisa con te.
    [many] tieni { $memories } di attività condivise con te.
   *[other] tieni { $memories } attività condivise con te.
}

journal.cli.social.status.shares = { $share_quantity ->
    [one] stai condividendo { $shares } attività.
    [many] stai condividendo { $shares } di attività.
   *[other] stai condividendo { $shares } attività.
}

journal.cli.social.status.spaces = { $space_quantity ->
    [one] stai seguendo { $spaces } Space.
    [many] stai seguendo { $spaces } di Space.
   *[other] stai seguendo { $spaces } Space.
}

journal.cli.social.sync.current = ogni attività condivisa è aggiornata.

journal.cli.social.sync.removed = { $activity_quantity ->
    [one] rimossa { $activities } attività condivisa che non è più nel tuo diario.
    [many] rimosse { $activities } di attività condivise che non sono più nel tuo diario.
   *[other] rimosse { $activities } attività condivise che non sono più nel tuo diario.
}

journal.cli.social.sync.updated = { $activity_quantity ->
    [one] aggiornata { $activities } attività condivisa.
    [many] aggiornate { $activities } di attività condivise.
   *[other] aggiornate { $activities } attività condivise.
}

journal.cli.social.unshare.done = hai smesso di condividere quell'attività, ed è sparita da ogni Space in cui l'hai condivisa.

journal.cli.social.unshare.unknown = quell'attività non è condivisa.

journal.cli.social.usage = uso: { $syntax }{ $line_break }{ $line_break }condividi un'attività del tuo diario con le persone che scegli, tramite uno Space sul tuo account atproto, e segui gli Space che altri condividono con te. il tuo diario tiene ciò che condividono separato dai tuoi ricordi. solo build di sviluppo.

journal.cli.speakers.backfill.error.flags.mismatch = flag di backfill immutabili non corrispondenti per l'operazione { $operation_id }: atteso commit={ $expected_commit } reattribute={ $expected_reattribute } accumulation={ $expected_accumulation }, ottenuto commit={ $got_commit } reattribute={ $got_reattribute } accumulation={ $got_accumulation }

journal.cli.speakers.backfill.error.invalid.flags = flag di backfill non validi: { $detail }

journal.cli.speakers.backfill.error.journal.root = ammissione della radice del diario non riuscita: { $error }

journal.cli.speakers.backfill.error.ledger = operazione di registro non riuscita: { $error }

journal.cli.speakers.backfill.error.lock = lock di esecuzione del backfill non riuscito: { $error }

journal.cli.speakers.backfill.error.not.found = operazione di backfill non trovata: { $id }

journal.cli.speakers.backfill.error.scan = scansione di backfill non riuscita: { $error }

journal.cli.speakers.owner.voice.error.identity.invalid = l'identità configurata del titolare non è ammessa

journal.cli.speakers.owner.voice.error.no.candidate = Nessun candidato disponibile

journal.cli.speakers.owner.voice.error.no.saved.voice = Nessuna voce del titolare è salvata

journal.cli.speakers.owner.voice.error.stale.candidate = Versione del candidato non corrispondente o generazione obsoleta

journal.cli.speakers.pool.error.centroid.width = discrepanza nella larghezza del centroide del pool di candidati

journal.cli.speakers.pool.error.lock = lock del pool di candidati non riuscito: { $error }

journal.cli.speakers.pool.error.read = lettura del pool di candidati non riuscita: { $error }

journal.cli.speakers.pool.error.write = scrittura del pool di candidati non riuscita: { $error }

journal.cli.speakers.pool.refusal.days.unreadable = messa in pari dell'elenco delle voci rifiutata: non è stato possibile elencare i giorni del diario

journal.cli.speakers.pool.refusal.format.unsupported = messa in pari dell'elenco delle voci rifiutata: l'elenco delle voci è in un formato a cui questa versione non può aggiungere

journal.cli.speakers.pool.refusal.lock.timeout = messa in pari dell'elenco delle voci rifiutata: il lock dell'elenco delle voci è scaduto

journal.cli.speakers.pool.refusal.lock.timeout.kept = messa in pari dell'elenco delle voci rifiutata: il lock dell'elenco delle voci è scaduto; ciò che è stato aggiunto è conservato

journal.cli.speakers.pool.refusal.pool.unreadable = messa in pari dell'elenco delle voci rifiutata: non è stato possibile leggere l'elenco delle voci

journal.cli.speakers.pool.refusal.pool.write.failed = messa in pari dell'elenco delle voci rifiutata: non è stato possibile scrivere l'elenco delle voci

journal.cli.speakers.pool.refusal.pool.write.failed.kept = messa in pari dell'elenco delle voci rifiutata: non è stato possibile scrivere l'elenco delle voci; ciò che è stato aggiunto è conservato

journal.cli.speakers.pool.refusal.progress.invalid = messa in pari dell'elenco delle voci rifiutata: il file di avanzamento della messa in pari non è valido; rimuovere awareness/speaker_candidates_backfill.json dal tuo diario riavvia la messa in pari in modo sicuro

journal.cli.speakers.pool.refusal.progress.unreadable = messa in pari dell'elenco delle voci rifiutata: non è stato possibile leggere il file di avanzamento della messa in pari; controlla che la cartella awareness del tuo diario sia leggibile

journal.cli.speakers.pool.refusal.progress.write.failed = messa in pari dell'elenco delle voci rifiutata: non è stato possibile scrivere il file di avanzamento della messa in pari

journal.cli.speakers.pool.refusal.progress.write.failed.kept = messa in pari dell'elenco delle voci rifiutata: non è stato possibile scrivere il file di avanzamento della messa in pari; ciò che è stato aggiunto è conservato

journal.cli.stats.arguments = uso: { $syntax }{ $line_break }{ $command }: errore: argomenti non riconosciuti: { $argument }

journal.cli.stats.cause.facet = errore dello store delle facette: { $detail }

journal.cli.stats.cause.invalid.day = giorno non valido { $day }; attesa una data di calendario { $date_format }

journal.cli.stats.cause.io = non è possibile accedere a { $path }: { $detail }

journal.cli.stats.cause.path = errore di percorso del diario: { $detail }

journal.cli.stats.cause.read = errore di lettura del diario: { $detail }

journal.cli.stats.cause.schema = validazione delle statistiche non riuscita: { $schema } è { $actual }, atteso { $expected }

journal.cli.stats.cause.timestamp = validazione delle statistiche non riuscita: { $timestamp } deve essere non vuoto

journal.cli.stats.cause.validation = validazione delle statistiche non riuscita: { $detail }

journal.cli.stats.cause.write = errore di scrittura atomica del diario: { $detail }

journal.cli.stats.configuration = Errore nel caricamento della configurazione giornaliera del diario: { $error }

journal.cli.stats.day.cache = Salvataggio della cache del giorno non riuscito per { $day }: { $error }

journal.cli.stats.enumerate = Errore nell'enumerazione dei giorni del diario: { $error }

journal.cli.stats.help = uso: { $syntax }{ $line_break }{ $line_break }Scansiona un diario { $brand } e genera statistiche{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $no_cache_option }{ $gap_5 }Disattiva la cache per giorno (forza la riscansione di tutti i giorni){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }Attiva l'output dettagliato{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }Attiva il log di debug

journal.cli.stats.overrides = validazione delle statistiche non riuscita: non è stato possibile leggere gli override dei talenti da { $path }

journal.cli.stats.publication.durable = pubblicato; durabilità incerta: { $source }

journal.cli.stats.publication.observed = pubblicato; osservazione incerta: { $source }

journal.cli.stats.roots = journal-stats non riuscito: { $error }

journal.cli.stats.saved = Statistiche salvate in { $path }

journal.cli.stats.tokens.directory = Lettura della directory dei token non riuscita: { $path }

journal.cli.stats.tokens.json = Token { $json_format } non valido in { $path }: { $error }

journal.cli.stats.tokens.load = Caricamento della cache dei token non riuscito per { $path }: { $error }

journal.cli.stats.tokens.read = Lettura del token non riuscita per { $path }: { $error }

journal.cli.stats.tokens.save = Salvataggio della cache dei token non riuscito per { $path }: { $error }

journal.cli.stats.validate = Errore nella validazione di { $filename }: { $error }

journal.cli.stats.write = Errore nella scrittura di { $filename }: { $error }

journal.cli.stats.written = { $days ->
    [one] Scritte le statistiche per { $days } giorno in { $path }
    [many] Scritte le statistiche per { $days } di giorni in { $path }
   *[other] Scritte le statistiche per { $days } giorni in { $path }
}

journal.cli.streams.arguments = uso: { $syntax }{ $break }{ $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.streams.empty = Nessun flusso trovato

journal.cli.streams.header.day = Ultimo giorno

journal.cli.streams.header.name = Nome

journal.cli.streams.header.segment = Ultimo segmento

journal.cli.streams.header.sequence = Sequenza

journal.cli.streams.header.type = Tipo

journal.cli.streams.help = uso: { $syntax }{ $paragraph }Ispeziona e gestisci l'identità del flusso{ $paragraph }argomenti posizionali:{ $break }{ $indent }{ $name_input }{ $name_space }Nome del flusso da ispezionare (ometti per elencare tutti i flussi){ $paragraph }opzioni:{ $break }{ $indent }{ $help_input }{ $help_space }mostra questo messaggio di aiuto ed esci{ $break }{ $indent }{ $rebuild_input }{ $rebuild_space }Ricostruisci lo stato del flusso dai marcatori per segmento{ $break }{ $indent }{ $verbose_input }{ $verbose_space }Attiva l'output dettagliato{ $break }{ $indent }{ $debug_input }{ $debug_space }Attiva il log di debug

journal.cli.streams.inspect.malformed = Non è stato possibile leggere il flusso { $path }: record malformato

journal.cli.streams.inspect.not.found = Flusso non trovato: { $name }

journal.cli.streams.list.error = Non è stato possibile elencare i flussi: { $error }

journal.cli.streams.marker.sequence = marcatore illeggibile { $path }: sequenza non valida

journal.cli.streams.marker.unreadable = marcatore illeggibile { $path }: non è stato possibile leggere il marcatore

journal.cli.streams.rebuild.empty = { $segment_quantity ->
    [one] Nessun flusso trovato ({ $segments } segmento scansionato)
    [many] Nessun flusso trovato ({ $segments } di segmenti scansionati)
   *[other] Nessun flusso trovato ({ $segments } segmenti scansionati)
}

journal.cli.streams.rebuild.result = { $stream_quantity ->
    [one] Ricostruito { $streams } flusso da { $segment_quantity ->
        [one] { $segments } segmento:
        [many] { $segments } di segmenti:
       *[other] { $segments } segmenti:
    }
    [many] Ricostruiti { $streams } di flussi da { $segment_quantity ->
        [one] { $segments } segmento:
        [many] { $segments } di segmenti:
       *[other] { $segments } segmenti:
    }
   *[other] Ricostruiti { $streams } flussi da { $segment_quantity ->
        [one] { $segments } segmento:
        [many] { $segments } di segmenti:
       *[other] { $segments } segmenti:
    }
}

journal.cli.streams.record.malformed = Non è stato possibile leggere il record del flusso { $path }: record malformato

journal.cli.streams.repair.ahead = { $stream }: la sequenza dello stato del flusso è avanti rispetto ai marcatori; invariato

journal.cli.streams.repair.locked = { $stream }: non è stato possibile bloccare { $path }; invariato

journal.cli.streams.repair.malformed = { $stream }: record del flusso malformato in { $path }; invariato

journal.cli.streams.repair.missing = { $stream }: nessun record del flusso in { $path }; invariato

journal.cli.streams.repair.write.failed = { $stream }: scrittura non riuscita per { $path }: pubblicazione atomica non riuscita

journal.cli.streams.segments.error = Non è stato possibile elencare i segmenti: { $error }

journal.cli.streams.supervisor = il diario non è in esecuzione. avvialo con '{ $command }' e riprova.

journal.cli.talent.compose.facet.missing = facetta '{ $facet }' non trovata

journal.cli.talent.compose.prompt.path.has.no.parent = talento { $talent }: il percorso del prompt non ha un genitore

journal.cli.talent.compose.prompt.path.is.missing = talento { $talent }: manca il percorso del prompt

journal.cli.talent.compose.schema.directory.is.unavailable = talento { $talent }: la directory dello schema non è disponibile: { $error }

journal.cli.talent.compose.schema.file.could.not.be.read = talento { $talent }: non è stato possibile leggere il file dello schema: { $error }

journal.cli.talent.compose.schema.file.is.not.a.valid.json.schema = talento { $talent }: il file dello schema non è un { $schema_format } valido: { $path }

journal.cli.talent.compose.schema.file.is.not.valid.json = talento { $talent }: il file dello schema non è un { $json } valido: { $path }

journal.cli.talent.compose.schema.file.not.found.cause = talento { $talent }: file dello schema non trovato: { $error }

journal.cli.talent.compose.schema.file.not.found.path = talento { $talent }: file dello schema non trovato: { $path }

journal.cli.talent.compose.schema.must.be.a.string.got = talento { $talent }: lo schema deve essere una stringa, ricevuto { $type_name }: { $value }

journal.cli.talent.compose.schema.path.escapes.talent.directory = talento { $talent }: il percorso dello schema esce dalla directory del talento: { $path }

journal.cli.talent.compose.schema.path.must.be.relative = talento { $talent }: il percorso dello schema deve essere relativo: { $path }

journal.cli.talent.compose.schema.path.must.not.contain = talento { $talent }: il percorso dello schema non deve contenere '..': { $path }

journal.cli.talent.compose.settings = non è stato possibile leggere la configurazione del diario: { $detail }

journal.cli.talent.config.access.retired = Prompt '{ $talent }' imposta { $access_tier } ritirato

journal.cli.talent.config.activities = Prompt pianificato per attività '{ $talent }' deve avere un elenco { $activities } non vuoto (tipi di attività da abbinare, o { $all_types } per tutti i tipi).

journal.cli.talent.config.budget.invalid = Prompt '{ $talent }' ha { $max_output_tokens } { $value } non valido. È atteso un intero positivo misurato rispetto al modello incluso.

journal.cli.talent.config.budget.required = Prompt '{ $talent }' non ha { $max_output_tokens } obbligatorio. Misura il suo output più grande sul modello incluso e dichiara 1,5 volte quel valore, arrotondato per eccesso a un multiplo di 256.

journal.cli.talent.config.budget.retired = Prompt '{ $talent }' dichiara { $thinking_budget } ritirato. Il ragionamento si imposta solo in Ragionamento, per il modello proprio del titolare.

journal.cli.talent.config.cwd.retired = Prompt '{ $talent }' imposta { $cwd } ritirato

journal.cli.talent.config.frontmatter = non è stato possibile analizzare il frontmatter da { $path }

journal.cli.talent.config.output.required = Prompt '{ $talent }' ha type={ $generate } ma manca il campo { $output } obbligatorio.

journal.cli.talent.config.priority = Prompt pianificato '{ $talent }' non ha il campo { $priority } obbligatorio. Tutti i prompt con { $schedule } devono dichiarare una priorità esplicita.

journal.cli.talent.config.read = non è stato possibile leggere { $path }: { $error }

journal.cli.talent.config.stat = non è stato possibile eseguire stat su { $path }: { $error }

journal.cli.talent.config.stem = il nome del file del talento non ha uno stem { $encoding }: { $path }

journal.cli.talent.config.type.invalid = Prompt '{ $talent }' ha un tipo non valido { $value }. È atteso { $generate }.

journal.cli.talent.config.type.required = Prompt '{ $talent }' ha un output ma manca il campo { $type } obbligatorio.

journal.cli.talent.daily.accepted.response = manca la risposta accettata

journal.cli.talent.daily.accepted.result = manca il risultato giornaliero accettato

journal.cli.talent.daily.artifact.before = conflitto: l'artefatto richiesto è cambiato dopo la preparazione del prompt

journal.cli.talent.daily.artifact.changed = conflitto: l'output richiesto è cambiato dopo la preparazione

journal.cli.talent.daily.artifact.snapshot = manca l'istantanea congelata dell'artefatto richiesto

journal.cli.talent.daily.artifact.snapshot.invalid = istantanea congelata dell'artefatto non valida: { $error }

journal.cli.talent.daily.attempt.day = manca il giorno del tentativo giornaliero

journal.cli.talent.daily.attempt.day.invalid = giorno del tentativo giornaliero non valido

journal.cli.talent.daily.attempt.mismatch = discrepanza nel { $key } giornaliero

journal.cli.talent.daily.attempt.token = manca il token del tentativo giornaliero

journal.cli.talent.daily.commit.observer = è atteso lo stato dei fatti dell'entità

journal.cli.talent.daily.commit.suggest = è atteso lo stato di suggerimento dell'entità

journal.cli.talent.daily.commit.text = è atteso l'output di testo

journal.cli.talent.daily.facet.changed = conflitto: la facetta proprietaria è cambiata dopo la preparazione del prompt

journal.cli.talent.daily.facet.identity = manca l'identità congelata della facetta

journal.cli.talent.daily.facet.path = percorso di output della facetta non valido

journal.cli.talent.daily.maintenance.changed = conflitto: l'orario giornaliero è cambiato dopo la preparazione del prompt

journal.cli.talent.daily.maintenance.output = output di manutenzione non valido: { $error }

journal.cli.talent.daily.maintenance.primary = l'output di manutenzione non ha primary

journal.cli.talent.daily.maintenance.snapshot = manca l'istantanea congelata dell'orario giornaliero

journal.cli.talent.daily.observer.changed = conflitto: l'osservazione è cambiata dopo la preparazione del prompt

journal.cli.talent.daily.observer.conflict = conflitto: { $error }

journal.cli.talent.daily.observer.entity.gone = conflitto: l'entità dell'osservazione è scomparsa

journal.cli.talent.daily.observer.validation = validazione: { $error }

journal.cli.talent.daily.output.outside = l'output giornaliero è fuori dal diario

journal.cli.talent.daily.output.path = percorso di output giornaliero non valido

journal.cli.talent.daily.owner.alias.claimed = conflitto: l'alias della promozione è stato rivendicato dopo la preparazione

journal.cli.talent.daily.owner.alias.target.detached = conflitto: la destinazione dell'alias non è più collegata

journal.cli.talent.daily.owner.identity.blocked.facet = conflitto: il nome della promozione è nascosto in questa facetta

journal.cli.talent.daily.owner.identity.blocked.hidden = conflitto: l'entità è nascosta, quindi i suoi nomi non sono stati cambiati

journal.cli.talent.daily.owner.identity.blocked.identity = conflitto: l'identità della promozione è nascosta

journal.cli.talent.daily.owner.identity.changed = conflitto: l'identità promossa è cambiata dopo la preparazione

journal.cli.talent.daily.owner.identity.deleted = conflitto: l'identità promossa è stata eliminata dopo la preparazione

journal.cli.talent.daily.owner.identity.disappeared = conflitto: l'identità della promozione è scomparsa

journal.cli.talent.daily.owner.identity.map.group.lost = conflitto: la promozione { $name } corrisponde a { $directory }, che ha perso il suo gruppo di identity-map

journal.cli.talent.daily.owner.identity.merged = conflitto: l'identità promossa è stata assorbita in una fusione dopo la preparazione

journal.cli.talent.daily.owner.identity.moved = conflitto: l'identità promossa è stata spostata dopo la preparazione

journal.cli.talent.daily.owner.identity.unreadable = conflitto: la promozione { $name } deriva l'id di entità { $slug }, la cui identità { $directory } non è leggibile

journal.cli.talent.daily.owner.merge.proposals.changed = conflitto: le proposte di fusione sono cambiate dopo la preparazione

journal.cli.talent.daily.owner.name.matches.multiple = conflitto: la promozione { $name } corrisponde a { $count } identità per nome

journal.cli.talent.daily.owner.no.usable.entity.id = conflitto: la promozione { $name } non ha un id di entità utilizzabile

journal.cli.talent.daily.owner.owning.facet.changed.absent = conflitto: la facetta proprietaria non esiste più

journal.cli.talent.daily.owner.owning.facet.changed.invalid = conflitto: la facetta proprietaria non ha un'identità stabile valida

journal.cli.talent.daily.owner.owning.facet.changed.replaced = conflitto: la facetta proprietaria è stata sostituita dopo la preparazione

journal.cli.talent.daily.owner.principal.grant.refused.exists = conflitto: esiste già un titolare del diario, quindi questa persona non è contrassegnata

journal.cli.talent.daily.owner.principal.grant.refused.name = conflitto: il nome configurato del titolare non corrisponde più a questa persona

journal.cli.talent.daily.owner.principal.settings = non è stato possibile leggere le impostazioni del diario, quindi questa persona non è stata contrassegnata come titolare

journal.cli.talent.daily.owner.promoted.entity.blocked.hidden = conflitto: l'entità promossa è nascosta

journal.cli.talent.daily.owner.promoted.entity.blocked.name = conflitto: l'entità promossa { $name } è nascosta

journal.cli.talent.daily.owner.relationship.changed = conflitto: la relazione della promozione è cambiata dopo la preparazione

journal.cli.talent.daily.owner.relationship.occupied = conflitto: la promozione { $name } richiede la directory di relazione { $relationship_dir }, che è già occupata

journal.cli.talent.daily.packet.absent = il tentativo giornaliero non ha un pacchetto congelato

journal.cli.talent.daily.packet.digest = discrepanza nel digest del pacchetto giornaliero

journal.cli.talent.daily.packet.scope = discrepanza di ambito del pacchetto giornaliero

journal.cli.talent.daily.packet.talent = discrepanza di talento del pacchetto giornaliero

journal.cli.talent.daily.parse.entities = le entità devono essere un array

journal.cli.talent.daily.parse.output = non è stato possibile analizzare l'output come JSON: { $error }

journal.cli.talent.daily.publication.plan = la pubblicazione richiede il risultato generato conservato e il piano preparato esatto

journal.cli.talent.daily.publication.record = manca il record di pubblicazione

journal.cli.talent.daily.response.malformed = la risposta giornaliera conservata è malformata

journal.cli.talent.daily.retry.scope = l'ambito della riprova giornaliera è cambiato

journal.cli.talent.daily.review.aliases = la promozione della revisione non ha alias

journal.cli.talent.daily.review.eligibility = manca l'idoneità congelata della revisione

journal.cli.talent.daily.review.hints = mancano i suggerimenti congelati della revisione

journal.cli.talent.daily.review.inputs = mancano gli input congelati di ammissione della revisione

journal.cli.talent.daily.review.merges = l'output della revisione non ha fusioni

journal.cli.talent.daily.review.name = al candidato congelato manca il nome

journal.cli.talent.daily.review.owner = manca lo stato congelato di revisione del titolare

journal.cli.talent.daily.review.promotion = manca lo stato congelato di promozione del titolare

journal.cli.talent.daily.review.promotion.changed = conflitto: lo stato di promozione del titolare è cambiato dopo la preparazione del prompt

journal.cli.talent.daily.review.promotions = l'output della revisione non ha promozioni

journal.cli.talent.daily.review.proposals = manca lo stato congelato della proposta

journal.cli.talent.daily.review.proposals.changed = conflitto: la proposta di unione è cambiata dopo la preparazione del prompt

journal.cli.talent.daily.review.type = il candidato congelato non ha il tipo

journal.cli.talent.daily.type = tipo di talento non supportato: { $kind }

journal.cli.talent.daily.validation.aliases = validazione: il candidato promosso della revisione deve includere l'array aliases

journal.cli.talent.daily.validation.json = validazione: JSON { $shape } non valido: { $error }

journal.cli.talent.daily.validation.maintenance.missing = validazione: manca il primario di manutenzione

journal.cli.talent.daily.validation.maintenance.time = validazione: il primario di manutenzione deve essere HH:MM

journal.cli.talent.daily.validation.merges = validazione: le unioni della revisione devono essere un array

journal.cli.talent.daily.validation.observer = validazione: entities:entity_observer richiede un array di entità

journal.cli.talent.daily.validation.promotions = validazione: le promozioni della revisione devono essere un array

journal.cli.talent.daily.validation.suggest = validazione: le entità di suggest devono essere un array

journal.cli.talent.daily.write.contract = contratto di scrittura giornaliero non supportato per { $talent }

journal.cli.talent.error.list.choice = uso: { $syntax }{ $line_break }{ $program }: errore: argomento { $flag }: scelta non valida: '{ $value }' (scegli tra { $choices })

journal.cli.talent.error.list.unknown = uso: { $syntax }{ $line_break }{ $program }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.talent.error.list.value = uso: { $syntax }{ $line_break }{ $program }: errore: argomento { $flag }: è atteso un argomento

journal.cli.talent.error.log.required = uso: { $syntax }{ $line_break }{ $program }: errore: i seguenti argomenti sono obbligatori: { $arguments }

journal.cli.talent.error.logs.count = uso: { $syntax }{ $line_break }{ $program }: errore: argomento { $count_options }: valore intero non valido: '{ $value }'

journal.cli.talent.error.logs.value = uso: { $syntax }{ $line_break }{ $program }: errore: argomento { $flag }: è atteso un argomento

journal.cli.talent.error.passages.required = uso: { $syntax }{ $line_break }{ $program }: errore: i seguenti argomenti sono obbligatori: { $arguments }

journal.cli.talent.error.passages.unknown = uso: { $syntax }{ $line_break }{ $program }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.talent.error.passages.value = uso: { $syntax }{ $line_break }{ $program }: errore: argomento { $flag }: è atteso un argomento

journal.cli.talent.error.root.choice = uso: { $syntax }{ $line_break }{ $program }: errore: argomento { $subcommand }: scelta non valida: '{ $value }' (scegli tra { $choices })

journal.cli.talent.error.root.encoding = uso: { $syntax }{ $line_break }{ $program }: errore: gli argomenti non sono validi come { $encoding }

journal.cli.talent.error.root.unknown = uso: { $syntax }{ $line_break }{ $program }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.talent.error.show.required = uso: { $syntax }{ $line_break }{ $program }: errore: i seguenti argomenti sono obbligatori: { $arguments }

journal.cli.talent.error.show.unknown = uso: { $syntax }{ $line_break }{ $program }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.talent.error.show.value = uso: { $syntax }{ $line_break }{ $program }: errore: argomento { $flag }: è atteso un argomento

journal.cli.talent.facet.malformed = dati malformati in { $path }

journal.cli.talent.facet.malformed.line = dati malformati in { $path } alla riga { $line }

journal.cli.talent.facet.object = la dichiarazione della facetta non è un oggetto: { $path }

journal.cli.talent.floor.invalid.budget = { $budget } mancante o non valido

journal.cli.talent.floor.mismatch = { $entries ->
    [one] conteggio degli elementi non coincidente: analizzato { $entries } elemento ma { $sources ->
        [one] { $sources } fonte di elementi
        [many] { $sources } di fonti di elementi
       *[other] { $sources } fonti di elementi
    }
    [many] conteggio degli elementi non coincidente: analizzati { $entries } di elementi ma { $sources ->
        [one] { $sources } fonte di elementi
        [many] { $sources } di fonti di elementi
       *[other] { $sources } fonti di elementi
    }
   *[other] conteggio degli elementi non coincidente: analizzati { $entries } elementi ma { $sources ->
        [one] { $sources } fonte di elementi
        [many] { $sources } di fonti di elementi
       *[other] { $sources } fonti di elementi
    }
}

journal.cli.talent.help.list = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $syntax_continuation_5 }{ $line_break }                        Filtra per tipo di pianificazione{ $line_break }{ $syntax_continuation_7 }{ $line_break }                        Filtra per origine{ $line_break }{ $indent_2 }{ $disabled_option }{ $gap_12 }Includi i prompt disattivati{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Produci come { $jsonl }

journal.cli.talent.help.log = uso: { $syntax }{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }  { $id }ID agente{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }Produci { $jsonl } grezzo{ $line_break }{ $indent_2 }{ $full_option }{ $gap_6 }Espandi i dettagli dell'evento

journal.cli.talent.help.logs = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }  { $agent }Filtra per un agente specifico{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $syntax_continuation_9 }{ $line_break }                        Numero di esecuzioni da mostrare (predefinito: 20){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Mostra solo le esecuzioni di questo giorno{ $line_break }{ $indent_2 }{ $daily_option }{ $gap_15 }Mostra solo le esecuzioni con pianificazione giornaliera{ $line_break }{ $indent_2 }{ $errors_option }{ $gap_14 }Mostra solo le esecuzioni con errore{ $line_break }{ $indent_2 }{ $summary_option }{ $gap_13 }Mostra il riepilogo raggruppato

journal.cli.talent.help.passages = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }                           nome{ $line_break }{ $line_break }Copia i passaggi esatti dalla fonte che un talento di attività legge.{ $line_break }Un'unità di fonte breve resta intera senza una chiamata al modello; una più lunga{ $line_break }richiede un completamento per scegliere cosa copiare. Nessun passaggio viene scritto nel{ $line_break }diario; l'uso di un completamento viene scritto nel log come qualunque altro.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }  { $name }Nome del talento di attività, come work o conversation{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Produci il documento dei passaggi come { $json }{ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Giorno dell'attività{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Facetta dell'attività{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }ID attività

journal.cli.talent.help.root = uso: { $syntax }{ $line_break }{ $line_break }Ispeziona le configurazioni dei prompt dei talenti{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $list }Elenca i prompt raggruppati per pianificazione{ $line_break }    { $show }Mostra i dettagli di un prompt specifico{ $line_break }    { $passages }Copia i passaggi della fonte di un'attività{ $line_break }    { $logs }Mostra il log recente delle esecuzioni del talento{ $line_break }    { $log }Mostra gli eventi di un'esecuzione di un agente{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Attiva l'output dettagliato{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Attiva il log di debug

journal.cli.talent.help.show = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                           nome{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }  { $name }Nome del prompt{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }Produci come { $jsonl }{ $line_break }{ $indent_2 }{ $prompt_option }{ $gap_14 }Mostra il contesto completo del prompt (modalità di prova a secco){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }Giorno per il contesto del prompt{ $line_break }{ $indent_2 }{ $segment_option }{ $gap_2 }Segmento per i prompt pianificati per segmento{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }Facetta per i prompt a più facette{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }ID attività per i prompt pianificati per attività{ $line_break }{ $indent_2 }{ $query_option }{ $gap_10 }Query di esempio per gli agenti strumento{ $line_break }{ $indent_2 }{ $full_option }{ $gap_16 }Mostra il contenuto completo senza troncamento

journal.cli.talent.hook.briefing.date.overflow = overflow della data del briefing

journal.cli.talent.hook.briefing.failed = pre-hook del briefing del mattino non riuscito: { $error }

journal.cli.talent.hook.budget = la richiesta preparata non entra nel budget di input della soglia

journal.cli.talent.hook.daily.lookback = rilettura della pianificazione giornaliera fuori da { $range }

journal.cli.talent.hook.day.invalid = giorno non valido o non interpretabile: '{ $day }'

journal.cli.talent.hook.day.missing = giorno mancante

journal.cli.talent.hook.document.stream = non è un segmento di importazione di un documento

journal.cli.talent.hook.entity.minimum = il pacchetto dell'entità { $entity } è di almeno { $count } caratteri; il massimo è { $maximum }

journal.cli.talent.hook.entity.name = nome dell'entità mancante

journal.cli.talent.hook.entity.size = il pacchetto dell'entità { $entity } è di { $count } caratteri; il massimo è { $maximum }

journal.cli.talent.hook.facet.missing = facetta mancante

journal.cli.talent.hook.facet.read = non è stato possibile leggere la facetta { $facet }: { $error }

journal.cli.talent.hook.facets.list = non è stato possibile elencare le facette: { $error }

journal.cli.talent.hook.facts.size = il contesto dei fatti dell'entità è di { $count } caratteri; il massimo è { $maximum }

journal.cli.talent.hook.failure = { $phase } hook '{ $stage }' per il talento '{ $talent }': { $detail }

journal.cli.talent.hook.identities.read = non è stato possibile leggere le identità dell'entità: { $error }

journal.cli.talent.hook.identity.object = l'identità dell'entità non è un oggetto: { $path }

journal.cli.talent.hook.lookup.control = componente di percorso non valido '{ $candidate }': il nome contiene un carattere di controllo

journal.cli.talent.hook.lookup.dotcomponent = componente di percorso non valido '{ $candidate }': { $dot } e { $parent } non sono nomi validi

journal.cli.talent.hook.lookup.empty = componente di percorso non valido '{ $candidate }': il nome è vuoto

journal.cli.talent.hook.lookup.kind = { $path } non è una directory ({ $kind })

journal.cli.talent.hook.lookup.layout = la disposizione diretta richiede il flusso "{ $default_stream }", ricevuto { $stream }

journal.cli.talent.hook.lookup.rootorprefix = componente di percorso non valido '{ $candidate }': il nome deve essere un solo componente relativo

journal.cli.talent.hook.lookup.separator = componente di percorso non valido '{ $candidate }': il nome contiene un separatore di percorso

journal.cli.talent.hook.possible.no.prompt = il talento non ha un prompt

journal.cli.talent.hook.resolution.absent = l'ambiguità risolta { $ambiguity } nomina l'entità { $entity } assente da questa chiamata di risoluzione

journal.cli.talent.hook.resolution.hidden = l'ambiguità risolta { $ambiguity } nomina l'entità nascosta { $entity }

journal.cli.talent.hook.review.changed = gli input della revisione sono cambiati durante la preparazione

journal.cli.talent.hook.stream.layout = la disposizione diretta richiede il flusso "{ $default_stream }", ricevuto "{ $stream }"

journal.cli.talent.hook.suggest.size = il contesto di suggest dell'entità è di { $count } caratteri; il massimo è { $maximum }

journal.cli.talent.hook.suggestions.array = nel JSON dei suggerimenti manca l'array { $entities }

journal.cli.talent.hook.suggestions.json = JSON dei suggerimenti non valido in { $path }: { $error }

journal.cli.talent.hook.suggestions.missing = artefatto dei suggerimenti mancante: { $path }

journal.cli.talent.hook.sunday = il giorno della richiesta { $day } non è una domenica

journal.cli.talent.hook.transcript.bytes = i byte della trascrizione non sono UTF-8 validi

journal.cli.talent.hook.transcript.read = non è stato possibile leggere la trascrizione { $file }: { $error }

journal.cli.talent.hook.transcript.utf8 = la trascrizione { $file } non è UTF-8: { $error }

journal.cli.talent.hook.week.incomplete = la settimana che finisce { $end } non è terminata prima di oggi { $today }

journal.cli.talent.last.age = { $age } fa

journal.cli.talent.last.age.duration = { $age } fa ({ $duration })

journal.cli.talent.list.empty = Nessun prompt trovato corrispondente ai filtri.

journal.cli.talent.list.group.activity = attività:

journal.cli.talent.list.group.cadence = cadenza:

journal.cli.talent.list.group.daily = giornaliero:

journal.cli.talent.list.group.segment = segmento:

journal.cli.talent.list.group.unscheduled = non pianificato:

journal.cli.talent.list.group.weekly = settimanale:

journal.cli.talent.list.header.last = ULTIMA ESECUZIONE

journal.cli.talent.list.header.name = NOME

journal.cli.talent.list.header.tags = ETICHETTE

journal.cli.talent.list.header.title = TITOLO

journal.cli.talent.list.tag.disabled = disattivato

journal.cli.talent.list.tag.fail = FALLITO

journal.cli.talent.list.visible = { $count ->
    [one] { $count } prompt ({ $hidden } disattivati nascosti, usa { $disabled_flag })
    [many] { $count } di prompt ({ $hidden } disattivati nascosti, usa { $disabled_flag })
   *[other] { $count } prompt ({ $hidden } disattivati nascosti, usa { $disabled_flag })
}

journal.cli.talent.log.event.thinking = ragionamento

journal.cli.talent.log.event.tool = strumento

journal.cli.talent.log.event.updated = aggiornato

journal.cli.talent.log.missing = Esecuzione del talento non trovata: { $run }

journal.cli.talent.log.tokens = { $result } [{ $input }in/{ $output }out]

journal.cli.talent.logs.date = { $month } { $day_time }

journal.cli.talent.logs.day.invalid = Formato { $day_flag } non valido: { $day }. Atteso { $day_format }.

journal.cli.talent.logs.total = totale

journal.cli.talent.outcome.facet.write = FacetEntityWriteError: { $error }

journal.cli.talent.outcome.object = il risultato non è un oggetto JSON

journal.cli.talent.outcome.observation.write = ObservationWriteError: { $error }

journal.cli.talent.outcome.observer.entities = le entità non sono una lista

journal.cli.talent.outcome.observer.parse = non è stato possibile analizzare il risultato come JSON

journal.cli.talent.outcome.review.arrays = nel risultato mancano gli array attesi

journal.cli.talent.outcome.review.candidate = EntityReviewCandidateError: { $error }

journal.cli.talent.outcome.review.parse = JSON non valido

journal.cli.talent.passages.generate = non è stato possibile raggiungere generate per i passaggi della fonte: { $error }

journal.cli.talent.passages.generate.missing = non è stato possibile raggiungere generate per i passaggi della fonte: manca l'eseguibile fratello { $path }

journal.cli.talent.passages.generate.parent = non è stato possibile raggiungere generate per i passaggi della fonte: l'eseguibile attuale non ha una directory padre

journal.cli.talent.passages.incomplete = alcune unità della fonte non hanno passaggi; vedi l'esito di ogni unità

journal.cli.talent.passages.unavailable = passaggi della fonte non disponibili: { $detail }

journal.cli.talent.passages.unit.failed = # unità { $index } · { $kind } · nessun passaggio: { $reason }

journal.cli.talent.passages.unit.failed.time = # unità { $index } · { $kind } · { $time } · nessun passaggio: { $reason }

journal.cli.talent.passages.unit.not.attempted = # unità { $index } · { $kind } · non tentata

journal.cli.talent.passages.unit.not.attempted.time = # unità { $index } · { $kind } · { $time } · non tentata

journal.cli.talent.passages.unit.passages = { $count ->
    [one] # unità { $index } · { $kind } · { $count } passaggio
    [many] # unità { $index } · { $kind } · { $count } di passaggi
   *[other] # unità { $index } · { $kind } · { $count } passaggi
}

journal.cli.talent.passages.unit.passages.time = { $count ->
    [one] # unità { $index } · { $kind } · { $time } · { $count } passaggio
    [many] # unità { $index } · { $kind } · { $time } · { $count } di passaggi
   *[other] # unità { $index } · { $kind } · { $time } · { $count } passaggi
}

journal.cli.talent.passages.verbatim = Alla lettera dalla fonte:

journal.cli.talent.path.absolute = il percorso del diario deve essere relativo

journal.cli.talent.path.backslash = il percorso del diario deve usare barre oblique

journal.cli.talent.path.component = il percorso del diario contiene un componente non valido

journal.cli.talent.path.day = il giorno deve essere { $day_format }

journal.cli.talent.path.empty = il percorso del diario non deve essere vuoto

journal.cli.talent.path.escape = { $relative } esce dalla radice del diario (risolto in { $path })

journal.cli.talent.path.unreadable = il percorso del diario non è leggibile

journal.cli.talent.prepare.configuration.name.declares.an.invalid.mobile.local.timeout = la configurazione '{ $name }' dichiara un { $timeout } non valido

journal.cli.talent.prepare.invalid.config.must.have.type.day.or.prompt = Configurazione non valida: deve avere { $type }, { $day } o { $prompt }

journal.cli.talent.prepare.invalid.config.segment.or.span.requires.day = Configurazione non valida: { $segment } o { $span } richiede { $day }

journal.cli.talent.prepare.invalid.talent.type.kind = tipo di talento non valido '{ $kind }'

journal.cli.talent.prepare.memory.incomplete = il contesto dei ricordi privati è incompleto

journal.cli.talent.prepare.request.field.field.is.retired = il campo della richiesta '{ $field }' è ritirato

journal.cli.talent.prepare.request.overrides.field.for.talent.name.are.not.allowed = Sovrascritture della richiesta '{ $field }' per il talento '{ $name }' non sono consentite ({ $declared } != { $requested })

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed = le sovrascritture della richiesta per '{ $field }' non sono consentite; provider e modello di ragionamento si configurano solo in Ragionamento

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed.budget = le sovrascritture della richiesta per '{ $field }' non sono consentite; i budget di un talento sono suoi

journal.cli.talent.prepare.segment.missing = { $count ->
    [one] Directory del segmento non trovata: { $segments }
    [many] Directory dei segmenti non trovate: { $segments }
   *[other] Directory dei segmenti non trovate: { $segments }
}

journal.cli.talent.prepare.talent.name.declares.field.in.frontmatter = il talento '{ $name }' dichiara '{ $field }' nel frontmatter; il provider e il modello di ragionamento si configurano solo in Ragionamento

journal.cli.talent.prepare.talent.name.declares.outbound.approval.in.frontmatter = il talento '{ $name }' dichiara { $outbound_approval } nel frontmatter; questo campo è solo della configurazione di avvio e non può provenire da una definizione di talento

journal.cli.talent.prepare.talent.name.not.found = talento '{ $name }' non trovato

journal.cli.talent.prepare.talent.request.missing.name = manca il nome nella richiesta del talento

journal.cli.talent.preview.all.lines = (usa { $full_flag } per vedere tutte le { $count } righe)

journal.cli.talent.preview.banner = Anteprima di ciò che questo talento invierebbe. Stesso assemblaggio che usa un'esecuzione reale; il modello non viene chiamato.

journal.cli.talent.preview.effective = Prompt effettivo per: { $talent }

journal.cli.talent.preview.empty = (vuoto)

journal.cli.talent.preview.instruction = ISTRUZIONE

journal.cli.talent.preview.no.day = Nessun giorno indicato, quindi questa anteprima non contiene le registrazioni di un giorno. Passa { $day_flag } { $day_format } per includerle.

journal.cli.talent.preview.omitted = { $count ->
    [one] ... ({ $count } riga omessa)
    [many] ... ({ $count } di righe omesse)
   *[other] ... ({ $count } righe omesse)
}

journal.cli.talent.preview.omitted.legacy = ... ({ $count } righe omesse)

journal.cli.talent.preview.prestep = L'anteprima del prompt non può assemblare questo talento: un passo preliminare obbligatorio non è disponibile.

journal.cli.talent.preview.recovery.choose.a.completed.owner.activity.not.an.anticipated.or.cogi = Scegli un'attività tua completata, non un record anticipated o cogitate.

journal.cli.talent.preview.recovery.choose.a.talent.configured.for.this.activity.kind = Scegli un talento configurato per questo tipo di attività.

journal.cli.talent.preview.recovery.choose.a.talent.that.loads.its.activity.s.sources = Scegli un talento che carica le fonti della sua attività.

journal.cli.talent.preview.recovery.choose.a.talent.that.reads.its.activity.source.without.a.pre = Scegli un talento che legge la fonte della sua attività senza un passo preliminare.

journal.cli.talent.preview.recovery.choose.a.talent.whose.required.pre.step.is.available.in.the = Scegli un talento il cui passo preliminare obbligatorio è disponibile nel runtime nativo.

journal.cli.talent.preview.recovery.choose.a.talent.whose.schedule.is.activity = Scegli un talento la cui pianificazione è { $schedule }

journal.cli.talent.preview.recovery.choose.an.activity.id.stored.in.the.requested.day.and.facet = Scegli un ID attività salvato nel giorno e nella facetta richiesti.

journal.cli.talent.preview.recovery.choose.an.activity.with.at.least.one.recorded.segment = Scegli un'attività con almeno un segmento preso.

journal.cli.talent.preview.recovery.choose.either.activity.or.segment.not.both = Scegli { $activity_flag } o { $segment_flag }, non entrambi.

journal.cli.talent.preview.recovery.correct.the.activity.preview.input.then.retry.detail = Correggi l'input dell'anteprima dell'attività, poi riprova: { $detail }

journal.cli.talent.preview.recovery.correct.the.talent.configuration.detail = Correggi la configurazione del talento: { $detail }

journal.cli.talent.preview.recovery.make.the.selected.activity.and.talent.eligible.then.retry = Rendi l'attività selezionata e il talento selezionato idonei, poi riprova.

journal.cli.talent.preview.recovery.make.the.selected.talent.eligible.then.retry = Rendi il talento selezionato idoneo, poi riprova.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.facet.name.and.activity.id = Passa { $day_flag } { $yyyymmdd }, { $facet_flag } { $name } e { $activity_flag } ID.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.with.activity = Passa { $day_flag } { $yyyymmdd } con { $activity_flag }.

journal.cli.talent.preview.recovery.pass.facet.name.with.activity = Passa { $facet_flag } { $name } con { $activity_flag }.

journal.cli.talent.preview.recovery.remove.duplicate.records.with.this.id.in.the.requested.day.a = Rimuovi i record duplicati con questo ID nel giorno e nella facetta richiesti.

journal.cli.talent.preview.recovery.repair.the.activity.span.so.every.segment.is.a.non.empty.str = Ripara lo span dell'attività in modo che ogni segmento sia una stringa non vuota.

journal.cli.talent.preview.recovery.restore.a.readable.activity.record.file.then.retry.detail = Ripristina un file leggibile del record dell'attività, poi riprova: { $detail }

journal.cli.talent.preview.recovery.restore.a.readable.segment.directory.then.retry.detail = Ripristina una directory leggibile del segmento, poi riprova: { $detail }

journal.cli.talent.preview.recovery.restore.readable.segment.directories.then.retry.detail = Ripristina directory leggibili dei segmenti, poi riprova: { $detail }

journal.cli.talent.preview.recovery.restore.the.selected.activity.segment.then.retry = Ripristina il segmento di attività selezionato, poi riprova.

journal.cli.talent.preview.skipped = Questo talento non verrebbe eseguito: { $reason }

journal.cli.talent.show.body = corpo:

journal.cli.talent.show.body.lines = { $count ->
    [one] { $count } riga
    [many] { $count } di righe
   *[other] { $count } righe
}

journal.cli.talent.show.hook = Il prompt '{ $talent }' è un prompt hook e non può essere eseguito direttamente.

journal.cli.talent.show.missing = Prompt non trovato: { $talent }

journal.cli.talent.show.missing.path = Prompt non trovato: { $talent }{ $line_break }  cercato in: { $path }

journal.cli.talent.show.variables = variabili:

journal.cli.talent.startup.executable = talento non riuscito: non è stato possibile ispezionare l'eseguibile attuale: { $error }

journal.cli.talent.startup.package = talento non riuscito: non è stato possibile trovare il pacchetto { $package } installato

journal.cli.talent.work.duration = durata della fonte Work non valida: { $error }

journal.cli.talent.work.duration.range = La durata della fonte Work è fuori intervallo

journal.cli.talent.work.interval = intervallo della fonte Work non valido: { $error }

journal.cli.talent.work.interval.range = L'intervallo della fonte Work è fuori intervallo

journal.cli.talent.work.key = segmento della fonte Work non valido: { $segment }

journal.cli.talent.work.read = non è possibile leggere i risultati del segmento Work: { $error }

journal.cli.talent.work.unavailable = Il segmento della fonte Work non è disponibile: { $day }/{ $segment }

journal.cli.top.crashed.attempts = { $name } (tentativi: { $attempts })

journal.cli.top.crashed.crashed = { $name } (Crash; tentativi: { $attempts })

journal.cli.top.crashed.heading.attention = Servizi che richiedono attenzione:

journal.cli.top.crashed.heading.crashed = Crash:

journal.cli.top.crashed.retrying = { $name } (Riprova in corso; tentativi: { $attempts })

journal.cli.top.error.apply = non è stato possibile applicare il terminale: { $detail }

journal.cli.top.error.journal.config = configurazione del diario: { $detail }

journal.cli.top.error.screen = non è stato possibile entrare nello schermo del terminale: { $detail }

journal.cli.top.error.stdin.not.tty = stdin non è un terminale

journal.cli.top.error.stdout.not.tty = stdout non è un terminale

journal.cli.top.error.terminal = guasto del terminale: { $detail }

journal.cli.top.error.transport = guasto di trasporto: { $detail }

journal.cli.top.error.width = non è stato possibile ottenere la larghezza del terminale: { $detail }

journal.cli.top.error.width.unavailable = larghezza del terminale non disponibile

journal.cli.top.footer.empty = q: Esci

journal.cli.top.footer.multi = ↑/↓: Naviga  r: Riavvia  q: Esci

journal.cli.top.footer.single = r: Riavvia  q: Esci

journal.cli.top.header.last.log = Ultimo log

journal.cli.top.header.mb = MB

journal.cli.top.header.percent = %

journal.cli.top.header.pid = PID

journal.cli.top.header.runtime = Tempo di esecuzione

journal.cli.top.header.service = Servizio

journal.cli.top.header.task = Compito

journal.cli.top.header.uptime = Tempo attivo

journal.cli.top.malformed.events = eventi malformati: { $count } ({ $reason })

journal.cli.top.observe.describe = descrizione

journal.cli.top.observe.idle = [INATTIVO]

journal.cli.top.observe.idle.locked = [INATTIVO] bloccato

journal.cli.top.observe.live.badge = [IN DIRETTA]

journal.cli.top.observe.recent = Recenti: { $segments }

journal.cli.top.observe.screencast = screencast { $elapsed }

journal.cli.top.observe.tmux.badge = [TMUX]

journal.cli.top.observe.tmux.captures = { $quantity ->
    [one] { $count } fotogramma
    [many] { $count } di fotogrammi
   *[other] { $count } fotogrammi
}

journal.cli.top.observe.transcribe = trascrizione

journal.cli.top.observe.voice = voce { $hits }

journal.cli.top.queued = in coda: { $commands }

journal.cli.top.reconnecting = (riconnessione in corso)

journal.cli.top.section.observe = Osservazione

journal.cli.top.section.think = Ragionamento

journal.cli.top.section.thinking.health = Stato del ragionamento

journal.cli.top.task.status.failed = non riuscito

journal.cli.top.task.status.gone = assente

journal.cli.top.task.status.ok = ok

journal.cli.top.think.agents = { $total_quantity ->
    [one] { $completed }/{ $total } esecuzione
    [many] { $completed }/{ $total } di esecuzioni
   *[other] { $completed }/{ $total } esecuzioni
}

journal.cli.top.think.last.failed = { $failed } non riuscito

journal.cli.top.think.last.prefix = Ultima: { $success } ok

journal.cli.top.think.segments = seg { $completed }/{ $total }

journal.cli.top.thinking.checking = (controllo in corso)

journal.cli.top.thinking.unavailable = (stato non disponibile)

journal.cli.top.title = solstone gestore delle attività

journal.cli.top.unknown = sconosciuto

journal.cli.top.waiting.services = (in attesa dei servizi)

journal.cli.top.waiting.status = (in attesa dello stato)

journal.cli.top.waiting.think = (in attesa del ragionamento)
