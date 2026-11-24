journal.cli.about.os.release.unavailable = informazioni: { $release_file } non disponibile: { $error }

journal.cli.about.resource.unavailable = non è stato possibile leggere le informazioni del diario. controlla che il tuo diario sia in esecuzione, poi riprova.

journal.cli.about.usage.journal = uso: { $syntax }

journal.cli.about.usage.remote = uso: { $syntax }

journal.cli.check.disk.error = non è stato possibile verificare lo spazio libero in { $path }: { $message }

journal.cli.check.disk.free = { $gb } GB liberi in { $path } (ne servono 20 GB)

journal.cli.check.disk.low = il diario e i modelli locali hanno bisogno di almeno 20 GB liberi — { $path } ha { $gb } GB

journal.cli.check.disk.overflow = overflow dello spazio libero

journal.cli.check.feedback = Pensi che questa lettura sia sbagliata per la tua macchina? Ci farebbe piacere una patch — { $url }

journal.cli.check.gpu.absent = nessuna GPU utilizzabile trovata — i modelli locali inclusi hanno bisogno di una GPU hardware

journal.cli.check.gpu.absent.linux = nessuna GPU utilizzabile trovata — i modelli locali inclusi hanno bisogno di una GPU hardware con almeno 6 GB

journal.cli.check.gpu.cuda.integrity = errore di integrità dell'ambiente di esecuzione CUDA

journal.cli.check.gpu.low.linux = la GPU { $gpu } ha { $gb } GB — i modelli locali inclusi hanno bisogno di almeno 6 GB

journal.cli.check.gpu.low.windows = la GPU { $gpu } ha { $gb } GB; i modelli locali inclusi hanno bisogno di almeno 6 GB

journal.cli.check.gpu.nvidia = NVIDIA GPU con { $gb } GB

journal.cli.check.gpu.nvidia.cpu = NVIDIA GPU con { $gb } GB; un modello in esecuzione sulla tua GPU; la trascrizione in esecuzione sulla tua CPU su questa macchina

journal.cli.check.gpu.nvidia.low = la GPU NVIDIA ha { $gb } GB — i modelli locali inclusi hanno bisogno di almeno 6 GB

journal.cli.check.gpu.nvidia.unknown = NVIDIA GPU rilevata ma non è stato possibile leggere la sua memoria — la prontezza della GPU è sconosciuta

journal.cli.check.gpu.readiness.unknown = non è stato possibile determinare la prontezza della GPU: { $error }

journal.cli.check.gpu.render.permission = esiste un nodo di render della GPU sotto { $path } ma questo utente non può aprirlo — aggiungiti al gruppo render con `{ $repair }`, poi esci e accedi di nuovo ed esegui di nuovo `{ $command }`

journal.cli.check.gpu.unified = NVIDIA GPU con { $gb } GB (memoria unificata)

journal.cli.check.gpu.unified.cpu = NVIDIA GPU con { $gb } GB (memoria unificata); un modello in esecuzione sulla tua GPU; la trascrizione in esecuzione sulla tua CPU su questa macchina

journal.cli.check.gpu.unknown = non è stato possibile leggere la memoria GPU. il ragionamento locale richiede almeno 6 GB

journal.cli.check.gpu.vulkan = GPU Vulkan { $gpu } con { $gb } GB

journal.cli.check.gpu.vulkan.cpu = GPU Vulkan { $gpu } con { $gb } GB; un modello in esecuzione sulla tua GPU; la trascrizione in esecuzione sulla tua CPU su questa macchina

journal.cli.check.gpu.vulkan.incomplete = nessuna GPU NVIDIA trovata e la sonda Vulkan non è terminata — la prontezza della GPU è sconosciuta

journal.cli.check.label.ced = ced

journal.cli.check.label.disk = disco

journal.cli.check.label.gpu = gpu

journal.cli.check.label.memory = memoria

journal.cli.check.label.platform = piattaforma

journal.cli.check.label.ram = ram

journal.cli.check.label.rfdetr = rfdetr

journal.cli.check.marker.blocked = [bloccato]

journal.cli.check.marker.ok = [ok]

journal.cli.check.marker.unknown = [sconosciuto]

journal.cli.check.marker.warn = [avviso]

journal.cli.check.memory.available = memoria totale sufficiente, ma solo { $gb } GB sono liberi ora — chiudi qualche app prima che i modelli locali si carichino

journal.cli.check.memory.eligible = { $gb } GB di memoria soddisfano il minimo di 16 GB

journal.cli.check.memory.minimum = Apple Silicon ha bisogno di almeno 16 GB di memoria per i modelli locali inclusi (questo Mac ha { $gb } GB)

journal.cli.check.memory.unknown = non è stato possibile verificare la memoria disponibile

journal.cli.check.overall.blocked = Non pronto — questo computer non può ancora eseguire i modelli locali inclusi.

journal.cli.check.overall.ready = Pronto — poi installa il diario:  vedi { $file }

journal.cli.check.overall.warning = Quasi pronto (vedi gli avvisi sopra) — puoi installare il diario:  vedi { $file }

journal.cli.check.platform.apple = Apple Silicon macOS (arm64)

journal.cli.check.platform.intel = I Mac Intel non sono supportati — usa un Mac Apple Silicon, oppure esegui il diario su Linux supportato.

journal.cli.check.platform.linux = Linux ({ $arch })

journal.cli.check.platform.unsupported = { $os }/{ $arch } non può ancora eseguire i modelli locali inclusi. le piattaforme supportate sono apple silicon macos, x86_64/aarch64 linux e x86_64/aarch64 windows.

journal.cli.check.platform.windows = Windows ({ $arch })

journal.cli.check.ram.low = { $gb } GB di memoria di sistema sono pochi — consigliati 8 GB o più

journal.cli.check.ram.total = { $gb } GB di memoria di sistema

journal.cli.check.ram.unknown = non è stato possibile verificare la memoria di sistema

journal.cli.check.title = { $command } — questo computer può eseguire il diario con i modelli locali inclusi?

journal.cli.doctor.backlog.band = { $recording_count ->
    [one] banda { $band }: { $recordings } registrazione pronta / { $recordings_left } rimanenti, { $outputs } { $output_count ->
        [one] output giornaliero pronto
        [many] di output giornalieri pronti
       *[other] output giornalieri pronti
    } / { $outputs_left } rimanenti, { $activities } { $activity_count ->
        [one] attività rimanente
        [many] di attività rimanenti
       *[other] attività rimanenti
    }, { $requests } { $request_count ->
        [one] richiesta tua
        [many] di richieste tue
       *[other] richieste tue
    }, { $permission } in attesa di permesso, { $prerequisites } in attesa di prerequisiti
    [many] banda { $band }: { $recordings } di registrazioni pronte / { $recordings_left } rimanenti, { $outputs } { $output_count ->
        [one] output giornaliero pronto
        [many] di output giornalieri pronti
       *[other] output giornalieri pronti
    } / { $outputs_left } rimanenti, { $activities } { $activity_count ->
        [one] attività rimanente
        [many] di attività rimanenti
       *[other] attività rimanenti
    }, { $requests } { $request_count ->
        [one] richiesta tua
        [many] di richieste tue
       *[other] richieste tue
    }, { $permission } in attesa di permesso, { $prerequisites } in attesa di prerequisiti
   *[other] banda { $band }: { $recordings } registrazioni pronte / { $recordings_left } rimanenti, { $outputs } { $output_count ->
        [one] output giornaliero pronto
        [many] di output giornalieri pronti
       *[other] output giornalieri pronti
    } / { $outputs_left } rimanenti, { $activities } { $activity_count ->
        [one] attività rimanente
        [many] di attività rimanenti
       *[other] attività rimanenti
    }, { $requests } { $request_count ->
        [one] richiesta tua
        [many] di richieste tue
       *[other] richieste tue
    }, { $permission } in attesa di permesso, { $prerequisites } in attesa di prerequisiti
}

journal.cli.doctor.backlog.caught = tutto in pari

journal.cli.doctor.backlog.caught.capped = { $day_count ->
    [one] tutto in pari; { $days } giorno completato con { $unit_count ->
        [one] un'unità giornaliera limitata
        [many] unità giornaliere limitate
       *[other] unità giornaliere limitate
    }
    [many] tutto in pari; { $days } di giorni completati con { $unit_count ->
        [one] un'unità giornaliera limitata
        [many] unità giornaliere limitate
       *[other] unità giornaliere limitate
    }
   *[other] tutto in pari; { $days } giorni completati con { $unit_count ->
        [one] un'unità giornaliera limitata
        [many] unità giornaliere limitate
       *[other] unità giornaliere limitate
    }
}

journal.cli.doctor.backlog.change.recorded = { $name } ha registrato una modifica su { $location }; risolvi la modifica registrata prima che quel giorno venga rifatto

journal.cli.doctor.backlog.change.started = { $name } potrebbe aver avviato una modifica ma non ha confermato il completamento su { $location }; risolvi la modifica in corso prima di rielaborare

journal.cli.doctor.backlog.checked = lavoro recente controllato

journal.cli.doctor.backlog.checked.capped = { $day_count ->
    [one] lavoro recente controllato; { $days } giorno completato con { $unit_count ->
        [one] un'unità giornaliera limitata
        [many] unità giornaliere limitate
       *[other] unità giornaliere limitate
    }
    [many] lavoro recente controllato; { $days } di giorni completati con { $unit_count ->
        [one] un'unità giornaliera limitata
        [many] unità giornaliere limitate
       *[other] unità giornaliere limitate
    }
   *[other] lavoro recente controllato; { $days } giorni completati con { $unit_count ->
        [one] un'unità giornaliera limitata
        [many] unità giornaliere limitate
       *[other] unità giornaliere limitate
    }
}

journal.cli.doctor.backlog.deferred = { $name } si è fermato su { $location }; può riprovare in un'esecuzione successiva dopo che il giorno locale cambia

journal.cli.doctor.backlog.deferred.reason = { $name } si è fermato su { $location } con motivo { $reason }; può riprovare in un'esecuzione successiva dopo che il giorno locale cambia

journal.cli.doctor.backlog.eligible = { $name } si è fermato su { $location }; può riprovare alla prossima esecuzione idonea

journal.cli.doctor.backlog.eligible.reason = { $name } si è fermato su { $location } con motivo { $reason }; può riprovare alla prossima esecuzione idonea

journal.cli.doctor.backlog.empty.date = nessuna

journal.cli.doctor.backlog.exhausted = { $name } si è fermato su { $location } dopo il suo nuovo tentativo automatico; esegui { $command }

journal.cli.doctor.backlog.exhausted.reason = { $name } si è fermato su { $location } con motivo { $reason } dopo il suo nuovo tentativo automatico; esegui { $command }

journal.cli.doctor.backlog.hold.retry = { $day }: { $reason } (nuovo tentativo { $unit })

journal.cli.doctor.backlog.inventory.partial.current = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; accantonati: { $holds }
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; accantonati: { $holds }
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; accantonati: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.current.clear = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta
}

journal.cli.doctor.backlog.inventory.partial.last = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; ultimo avanzamento noto; accantonati: { $holds }
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; ultimo avanzamento noto; accantonati: { $holds }
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; ultimo avanzamento noto; accantonati: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.last.clear = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; ultimo avanzamento noto
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; ultimo avanzamento noto
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione incompleta; ultimo avanzamento noto
}

journal.cli.doctor.backlog.inventory.whole.current = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; accantonati: { $holds }
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; accantonati: { $holds }
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; accantonati: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.current.clear = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario
}

journal.cli.doctor.backlog.inventory.whole.last = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; ultimo avanzamento noto; accantonati: { $holds }
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; ultimo avanzamento noto; accantonati: { $holds }
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; ultimo avanzamento noto; accantonati: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.last.clear = { $quantity ->
    [one] ispezionato { $inspected } di { $total } giorno, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; ultimo avanzamento noto
    [many] ispezionati { $inspected } di { $total } di giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; ultimo avanzamento noto
   *[other] ispezionati { $inspected } di { $total } giorni, da { $oldest } a { $newest }, al { $as_of }; { $bands }; ispezione dell'intero diario; ultimo avanzamento noto
}

journal.cli.doctor.backlog.pending = in sospeso: { $days }

journal.cli.doctor.backlog.pending.counts = { $pending_count ->
    [one] { $pending } giorno in sospeso, { $stuck } { $stuck_count ->
        [one] giorno
        [many] di giorni
       *[other] giorni
    } in stallo
    [many] { $pending } di giorni in sospeso, { $stuck } { $stuck_count ->
        [one] giorno
        [many] di giorni
       *[other] giorni
    } in stallo
   *[other] { $pending } giorni in sospeso, { $stuck } { $stuck_count ->
        [one] giorno
        [many] di giorni
       *[other] giorni
    } in stallo
}

journal.cli.doctor.backlog.pending.oldest = { $pending_count ->
    [one] { $pending } giorno in sospeso, { $stuck } { $stuck_count ->
        [one] giorno
        [many] di giorni
       *[other] giorni
    } in stallo; il più vecchio ancora aperto { $day }
    [many] { $pending } di giorni in sospeso, { $stuck } { $stuck_count ->
        [one] giorno
        [many] di giorni
       *[other] giorni
    } in stallo; il più vecchio ancora aperto { $day }
   *[other] { $pending } giorni in sospeso, { $stuck } { $stuck_count ->
        [one] giorno
        [many] di giorni
       *[other] giorni
    } in stallo; il più vecchio ancora aperto { $day }
}

journal.cli.doctor.backlog.resolve = { $name } si è fermato su { $location }; risolvi il conflitto prima di rielaborare un giorno passato

journal.cli.doctor.backlog.resolve.reason = { $name } si è fermato su { $location } con motivo { $reason }; risolvi il conflitto prima di rielaborare un giorno passato

journal.cli.doctor.backlog.retry = { $name } si è fermato su { $location }; esegui { $command }

journal.cli.doctor.backlog.retry.reason = { $name } si è fermato su { $location } con motivo { $reason }; esegui { $command }

journal.cli.doctor.backlog.review.started = { $name } potrebbe aver avviato una modifica di revisione dell'entità ma non ha confermato il completamento su { $location }; risolvi la modifica in corso prima di rielaborare

journal.cli.doctor.backlog.stuck = in stallo: { $days }

journal.cli.doctor.backlog.unfinished = { $activity_count ->
    [one] { $activities } attività su { $days } { $day_count ->
        [one] giorno completato
        [many] di giorni completati
       *[other] giorni completati
    } non è stato possibile finire l'elaborazione
    [many] { $activities } di attività su { $days } { $day_count ->
        [one] giorno completato
        [many] di giorni completati
       *[other] giorni completati
    } non è stato possibile finire l'elaborazione
   *[other] { $activities } attività su { $days } { $day_count ->
        [one] giorno completato
        [many] di giorni completati
       *[other] giorni completati
    } non è stato possibile finire l'elaborazione
}

journal.cli.doctor.backlog.unknown = sconosciuto: { $days }

journal.cli.doctor.backlog.unknown.count = { $quantity ->
    [one] non è stato possibile determinare del tutto; { $days } giorno sconosciuto
    [many] non è stato possibile determinare del tutto; { $days } di giorni sconosciuti
   *[other] non è stato possibile determinare del tutto; { $days } giorni sconosciuti
}

journal.cli.doctor.backup.association = { $association ->
    [unbound] associazione della configurazione sconosciuta; solo cronologia
    [unknown] associazione della configurazione attuale non disponibile
    [superseded] configurazione sostituita; solo cronologia
    [current] corrisponde alla configurazione selezionata
   *[invalid] associazione della configurazione registrata non disponibile
}

journal.cli.doctor.backup.attempt.incomplete = { $outcome ->
    [success] backup attivo; ultimo tentativo registrato: copia riuscita il { $date }; alcuni file non sono stati copiati; { $history }; { $association }
    [incomplete] backup attivo; ultimo tentativo registrato: copia incompleta il { $date }; alcuni file non sono stati copiati; { $history }; { $association }
   *[failed] backup attivo; ultimo tentativo registrato: non riuscito il { $date }; alcuni file non sono stati copiati; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.locked = { $outcome ->
    [success] backup attivo; ultimo tentativo registrato: copia riuscita il { $date }; il repository era bloccato; { $history }; { $association }
    [incomplete] backup attivo; ultimo tentativo registrato: copia incompleta il { $date }; il repository era bloccato; { $history }; { $association }
   *[failed] backup attivo; ultimo tentativo registrato: non riuscito il { $date }; il repository era bloccato; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.ordinary = { $outcome ->
    [success] backup attivo; ultimo tentativo registrato: copia riuscita il { $date }; { $history }; { $association }
    [incomplete] backup attivo; ultimo tentativo registrato: copia incompleta il { $date }; { $history }; { $association }
   *[failed] backup attivo; ultimo tentativo registrato: non riuscito il { $date }; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.refused = { $outcome ->
    [success] backup attivo; ultimo tentativo registrato: copia riuscita il { $date }; chiave di backup rifiutata; { $history }; { $association }
    [incomplete] backup attivo; ultimo tentativo registrato: copia incompleta il { $date }; chiave di backup rifiutata; { $history }; { $association }
   *[failed] backup attivo; ultimo tentativo registrato: non riuscito il { $date }; chiave di backup rifiutata; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.timeout = { $outcome ->
    [success] backup attivo; ultimo tentativo registrato: copia riuscita il { $date }; tempo scaduto; { $history }; { $association }
    [incomplete] backup attivo; ultimo tentativo registrato: copia incompleta il { $date }; tempo scaduto; { $history }; { $association }
   *[failed] backup attivo; ultimo tentativo registrato: non riuscito il { $date }; tempo scaduto; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.unknown = { $outcome ->
    [success] backup attivo; ultimo tentativo registrato: copia riuscita il { $date }; causa sconosciuta; { $history }; { $association }
    [incomplete] backup attivo; ultimo tentativo registrato: copia incompleta il { $date }; causa sconosciuta; { $history }; { $association }
   *[failed] backup attivo; ultimo tentativo registrato: non riuscito il { $date }; causa sconosciuta; { $history }; { $association }
}

journal.cli.doctor.backup.date.unknown = data sconosciuta

journal.cli.doctor.backup.details = apri il backup o lo stato nel tuo diario per i dettagli

journal.cli.doctor.backup.history.incomplete.dated = { $quantity ->
    [one] ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età { $seconds } secondo; copia incompleta, alcuni file non sono stati copiati; copertura e verifica attuali sconosciute
    [many] ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età { $seconds } di secondi; copia incompleta, alcuni file non sono stati copiati; copertura e verifica attuali sconosciute
   *[other] ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età { $seconds } secondi; copia incompleta, alcuni file non sono stati copiati; copertura e verifica attuali sconosciute
}

journal.cli.doctor.backup.history.incomplete.undated = ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età sconosciuta; copia incompleta, alcuni file non sono stati copiati; copertura e verifica attuali sconosciute

journal.cli.doctor.backup.history.success.dated = { $quantity ->
    [one] ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età { $seconds } secondo; copertura e verifica attuali sconosciute
    [many] ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età { $seconds } di secondi; copertura e verifica attuali sconosciute
   *[other] ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età { $seconds } secondi; copertura e verifica attuali sconosciute
}

journal.cli.doctor.backup.history.success.undated = ultima copia riuscita (storica): istantanea { $snapshot } il { $date }, età sconosciuta; copertura e verifica attuali sconosciute

journal.cli.doctor.backup.history.unknown = ultima copia riuscita sconosciuta

journal.cli.doctor.backup.invalid.history = backup attivo; stato registrato non disponibile; { $history }

journal.cli.doctor.backup.paused.history = backup in pausa da te dal { $date }; { $history }

journal.cli.doctor.backup.snapshot.unknown = sconosciuta

journal.cli.doctor.backup.status.invalid.pause = stato del backup non disponibile: impostazione di pausa non valida

journal.cli.doctor.backup.status.invalid.settings = stato del backup non disponibile: impostazioni di backup non valide

journal.cli.doctor.backup.status.invalid.switch = stato del backup non disponibile: impostazione di attivo/disattivo non valida

journal.cli.doctor.backup.status.no.attempt = backup attivo; nessun tentativo registrato

journal.cli.doctor.backup.status.off = backup disattivo

journal.cli.doctor.backup.status.unavailable.journal = stato del backup non disponibile: diario illeggibile

journal.cli.doctor.backup.status.unavailable.settings = stato del backup non disponibile: impostazioni illeggibili

journal.cli.doctor.backup.status.unconfigured = backup non configurato

journal.cli.doctor.brain.age.unknown = sconosciuta

journal.cli.doctor.brain.no.component = nessuno

journal.cli.doctor.brain.report = { $headline }; { $state_key }={ $state }; { $reason_key }={ $reason }; { $component_key }={ $component }; { $age_key }={ $age }

journal.cli.doctor.brain.unknown = sconosciuto: stato di brain non disponibile

journal.cli.doctor.brain.unknown.reason = sconosciuto: { $reason }

journal.cli.doctor.capture.health.active = { $rollup }; l'app di { $brand } su ogni dispositivo che ha aggiunto qualcosa al tuo diario è aggiornata

journal.cli.doctor.capture.health.data.unavailable = { $rollup }; attività del dispositivo non disponibile

journal.cli.doctor.capture.health.inspect = apri { $health_path } per ispezionare ogni dispositivo

journal.cli.doctor.capture.health.no.senders = { $rollup }; l'app di { $brand } non ha ancora aggiunto niente al tuo diario

journal.cli.doctor.capture.health.quiet = { $rollup }; nessun caricamento rifiutato registrato; vedi i dispositivi per gli orari dell'ultima consegna

journal.cli.doctor.capture.health.registry.unavailable = { $rollup }; elenco dei dispositivi non disponibile

journal.cli.doctor.capture.recent = l'app di { $brand } su { $client } ha aggiunto l'ultima volta { $hours } h fa

journal.cli.doctor.capture.recent.sources = { $quantity ->
    [one] l'app di { $brand } su { $client } ha aggiunto l'ultima volta { $hours } h fa ({ $sources } richiede attenzione)
    [many] l'app di { $brand } su { $client } ha aggiunto l'ultima volta { $hours } h fa ({ $sources } richiedono attenzione)
   *[other] l'app di { $brand } su { $client } ha aggiunto l'ultima volta { $hours } h fa ({ $sources } richiedono attenzione)
}

journal.cli.doctor.capture.rollup = { $rollup }; { $devices }

journal.cli.doctor.capture.sources = l'app di { $brand } su { $client } ha problemi ad aggiungere { $sources }

journal.cli.doctor.capture.trouble = l'app di { $brand } su { $client } ha problemi ad aggiungere

journal.cli.doctor.capture.trouble.sources = { $quantity ->
    [one] l'app di { $brand } su { $client } ha problemi ad aggiungere ({ $sources } richiede attenzione)
    [many] l'app di { $brand } su { $client } ha problemi ad aggiungere ({ $sources } richiedono attenzione)
   *[other] l'app di { $brand } su { $client } ha problemi ad aggiungere ({ $sources } richiedono attenzione)
}

journal.cli.doctor.check.deferred = insieme di controlli rinviati: { $set }

journal.cli.doctor.client.binding.active = registri client attivi={ $records }; non associati=0; associazione { $certificate_term } è autorevole

journal.cli.doctor.client.delivery.stall.activity.unavailable = attività del dispositivo non disponibile

journal.cli.doctor.client.delivery.stall.inspect = apri { $health_path } per ispezionare il caricamento rifiutato

journal.cli.doctor.client.delivery.stall.list.unavailable = elenco dei dispositivi non disponibile

journal.cli.doctor.client.delivery.stall.no.senders = l'app di { $brand } non ha ancora aggiunto niente al tuo diario

journal.cli.doctor.client.delivery.stall.quiet = nessun caricamento rifiutato registrato; vedi i dispositivi per gli orari dell'ultima consegna

journal.cli.doctor.client.ingest.health.healthy = nessun dispositivo con ingresso non riuscito

journal.cli.doctor.client.ingest.health.rejection = dispositivo { $device } con ingresso non riuscito: { $reason }, { $attempts }x da { $since }

journal.cli.doctor.client.ingest.health.repair = aggiorna o riavvia il dispositivo, poi conferma che un caricamento valido risolve il rifiuto

journal.cli.doctor.client.transport.refusal.healthy = nessun dispositivo ha avuto richieste respinte negli ultimi 7 giorni

journal.cli.doctor.client.transport.refusal.repair = questa è una connessione occupata, non una persa; segnalala se continua a succedere

journal.cli.doctor.clients.ledger.unavailable = registro dei client autorizzati non disponibile: { $reason }

journal.cli.doctor.config.dir.readable.config.unwritable = la directory di configurazione del servizio non è scrivibile: { $path }

journal.cli.doctor.config.dir.readable.config.will.create = la directory personale è scrivibile; l'installazione creerà { $path }

journal.cli.doctor.config.dir.readable.config.writable = la directory personale e di configurazione del servizio sono scrivibili ({ $path })

journal.cli.doctor.config.dir.readable.home.missing = la directory personale non esiste: { $path }

journal.cli.doctor.config.dir.readable.home.unusable = la directory personale non è leggibile e scrivibile: { $path }

journal.cli.doctor.config.dir.readable.owner.unavailable = non è stato possibile individuare la base del titolare: { $error }

journal.cli.doctor.config.dir.readable.owner.windows = la base del titolare per Windows è leggibile solo su Windows

journal.cli.doctor.config.restore = ripara o ripristina { $config_file } da un backup

journal.cli.doctor.context.failed = non è stato possibile risolvere il contesto di doctor: { $error }

journal.cli.doctor.default.stt.ready.backend = il backend configurato è { $backend }; prontezza di parakeet non applicabile

journal.cli.doctor.default.stt.ready.download = il modello parakeet { $coreml } non è scaricato — scaricalo con: { $models_command }

journal.cli.doctor.default.stt.ready.other.model.ready = modello parakeet pronto in { $path }

journal.cli.doctor.default.stt.ready.unsupported = parakeet non supportato su questa piattaforma

journal.cli.doctor.delivery.rejected = il diario ha rifiutato un caricamento da { $client }

journal.cli.doctor.delivery.rejected.sources = { $quantity ->
    [one] il diario ha rifiutato un caricamento da { $client } ({ $sources } richiede attenzione)
    [many] il diario ha rifiutato un caricamento da { $client } ({ $sources } richiedono attenzione)
   *[other] il diario ha rifiutato un caricamento da { $client } ({ $sources } richiedono attenzione)
}

journal.cli.doctor.device.day.listing.chronicle.error = flussi del dispositivo non disponibili: chronicle illeggibile: { $error }

journal.cli.doctor.device.day.listing.continuity.error = flussi del dispositivo non disponibili: continuità del flusso illeggibile: { $error }

journal.cli.doctor.device.day.listing.continuity.malformed = flussi del dispositivo non disponibili: continuità del flusso illeggibile: il registro di continuità è malformato o non supportato

journal.cli.doctor.device.day.listing.fault = giorno { $day } flusso { $stream } dispositivo { $device }: { $reason }

journal.cli.doctor.device.day.listing.readable = il diario può leggere gli ultimi { $days } giorni per ogni dispositivo associato

journal.cli.doctor.device.day.listing.record.line = flussi del dispositivo non disponibili: registro dei flussi illeggibile: registro di flusso malformato { $record }: dati malformati in { $path } alla riga { $line }

journal.cli.doctor.device.day.listing.record.malformed = flussi del dispositivo non disponibili: registro dei flussi illeggibile: registro di flusso malformato { $record }: dati malformati in { $path }

journal.cli.doctor.device.day.listing.registry.error = flussi del dispositivo non disponibili: registro dei flussi illeggibile: { $error }

journal.cli.doctor.device.day.listing.repair = a un dispositivo che sincronizza quel giorno si dice che il diario non può leggerlo; un'app di { $brand } più vecchia della correzione del 2026-09 lo mostra come non in linea; ispeziona i segmenti della directory del flusso nominata e il loro { $events_file }

journal.cli.doctor.device.day.listing.streams.unbound = nessun flusso di dispositivo associato

journal.cli.doctor.device.refusal.count = { $quantity ->
    [one] il dispositivo { $device } ha avuto una richiesta respinta: { $reason }, { $attempts }x fino a { $last_seen }
    [many] il dispositivo { $device } ha avuto richieste respinte: { $reason }, { $attempts }x fino a { $last_seen }
   *[other] il dispositivo { $device } ha avuto richieste respinte: { $reason }, { $attempts }x fino a { $last_seen }
}

journal.cli.doctor.devices.absent = nessun dispositivo registrato

journal.cli.doctor.devices.more = { $devices }{ $separator }+{ $extra } in più

journal.cli.doctor.devices.unavailable = registri dei dispositivi non disponibili: { $error }

journal.cli.doctor.disk.space.layout.unsupported = la radice di installazione risolta non è un albero di installazione layout

journal.cli.doctor.disk.space.package.install = un'installazione da pacchetto viene sostituita sul posto quando si aggiorna, quindi non serve una seconda copia

journal.cli.doctor.disk.space.root.unresolved = non è stato possibile risolvere la radice di installazione dalla directory bin d'installazione: { $path }

journal.cli.doctor.disk.space.size.overflow = overflow della dimensione dell'albero di installazione in { $path }

journal.cli.doctor.disk.space.size.unreadable = non è stato possibile misurare l'albero di installazione in { $path }: { $error }

journal.cli.doctor.disk.space.space.available = { $free_gib } { $gib } liberi sul filesystem che contiene { $path } ({ $required_gib } { $gib } è una seconda copia di questa installazione più un margine)

journal.cli.doctor.disk.space.space.inspect = non è stato possibile ispezionare lo spazio libero in { $path }: { $error }

journal.cli.doctor.disk.space.space.low = solo { $free_gib } { $gib } liberi sul filesystem che contiene { $path }; { $required_gib } { $gib } è una seconda copia di questa installazione più un margine

journal.cli.doctor.disk.space.space.unreadable = lo spazio libero non è leggibile in { $path }

journal.cli.doctor.durability.action.recovered = non serve alcuna azione: i registri ripristinati restano accanto agli originali e sono elencati qui

journal.cli.doctor.durability.action.refused = il diario non si avvia finché questo non si risolve; ogni riga sopra dice se si risolve dopo il riavvio del computer o se serve aiuto

journal.cli.doctor.durability.action.unreadable = controlla i percorsi sotto 'non è stato possibile ispezionare'; i registri ripristinati restano accanto agli originali e sono elencati qui

journal.cli.doctor.durability.admission.helper = { $finding ->
    [exited] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, uscito
    [escalated] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, ritirato (in escalation: true)
    [retired] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, ritirato
    [unacknowledged] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, mai riconosciuto
    [spawn] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, avvio non riuscito
    [unidentified] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, non identificato
    [rejected] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, rifiutato e raccolto
   *[reused] generazione { $generation } ammissione { $admission }: ausiliario { $identity }, pid riutilizzato da un altro utente
}

journal.cli.doctor.durability.admission.service = { $finding ->
    [exited] generazione { $generation } ammissione { $admission }: servizio { $identity }, uscito
    [escalated] generazione { $generation } ammissione { $admission }: servizio { $identity }, ritirato (in escalation: true)
    [retired] generazione { $generation } ammissione { $admission }: servizio { $identity }, ritirato
    [unacknowledged] generazione { $generation } ammissione { $admission }: servizio { $identity }, mai riconosciuto
    [spawn] generazione { $generation } ammissione { $admission }: servizio { $identity }, avvio non riuscito
    [unidentified] generazione { $generation } ammissione { $admission }: servizio { $identity }, non identificato
    [rejected] generazione { $generation } ammissione { $admission }: servizio { $identity }, rifiutato e raccolto
   *[reused] generazione { $generation } ammissione { $admission }: servizio { $identity }, pid riutilizzato da un altro utente
}

journal.cli.doctor.durability.clean = nessun ripristino registrato da segnalare

journal.cli.doctor.durability.clean.outstanding = { $quantity ->
    [one] nessun ripristino registrato da segnalare; l'esecuzione più recente (generazione { $generation }) deve ancora { $launches } avvio
    [many] nessun ripristino registrato da segnalare; l'esecuzione più recente (generazione { $generation }) deve ancora { $launches } di avvii
   *[other] nessun ripristino registrato da segnalare; l'esecuzione più recente (generazione { $generation }) deve ancora { $launches } avvii
}

journal.cli.doctor.durability.closed = { $quantity ->
    [one] la generazione { $generation } è stata chiusa dall'avvio che è diventato la generazione { $successor } (unix { $time }): { $total } ammissione, { $exited } già uscita, { $retired } fermata dal diario, { $never } mai ammessa
    [many] la generazione { $generation } è stata chiusa dall'avvio che è diventato la generazione { $successor } (unix { $time }): { $total } di ammissioni, { $exited } già uscite, { $retired } fermate dal diario, { $never } mai ammesse
   *[other] la generazione { $generation } è stata chiusa dall'avvio che è diventato la generazione { $successor } (unix { $time }): { $total } ammissioni, { $exited } già uscite, { $retired } fermate dal diario, { $never } mai ammesse
}

journal.cli.doctor.durability.closed.restarted = { $quantity ->
    [one] la generazione { $generation } è stata chiusa dall'avvio che è diventato la generazione { $successor } (unix { $time }): { $total } ammissione, { $exited } già uscita, { $retired } fermata dal diario, { $never } mai ammessa; il computer era stato riavviato nel frattempo
    [many] la generazione { $generation } è stata chiusa dall'avvio che è diventato la generazione { $successor } (unix { $time }): { $total } di ammissioni, { $exited } già uscite, { $retired } fermate dal diario, { $never } mai ammesse; il computer era stato riavviato nel frattempo
   *[other] la generazione { $generation } è stata chiusa dall'avvio che è diventato la generazione { $successor } (unix { $time }): { $total } ammissioni, { $exited } già uscite, { $retired } fermate dal diario, { $never } mai ammesse; il computer era stato riavviato nel frattempo
}

journal.cli.doctor.durability.heading.aside = scritture accantonate ({ $count }):

journal.cli.doctor.durability.heading.closed = esecuzioni chiuse da un avvio successivo ({ $count }):

journal.cli.doctor.durability.heading.refused = avvii rifiutati ({ $count }):

journal.cli.doctor.durability.heading.unreadable = non è stato possibile ispezionare ({ $count }):

journal.cli.doctor.durability.heading.unresolved = esecuzioni che non si sono fermate in modo pulito ({ $count }):

journal.cli.doctor.durability.more = ... e altri { $count }

journal.cli.doctor.durability.outstanding = { $quantity ->
    [one] l'esecuzione più recente (generazione { $generation }) deve ancora { $launches } avvio
    [many] l'esecuzione più recente (generazione { $generation }) deve ancora { $launches } di avvii
   *[other] l'esecuzione più recente (generazione { $generation }) deve ancora { $launches } avvii
}

journal.cli.doctor.durability.refusal.restart = generazione { $generation }: il registro dei processi avviati da un ritiro è perso, quindi il diario non si avvia; si risolve dopo il riavvio del computer

journal.cli.doctor.durability.refusal.support = generazione { $generation }: il registro dei processi avviati da un ritiro è perso, quindi il diario non si avvia; non si risolve da sé e serve aiuto dal supporto

journal.cli.doctor.durability.unresolved = generazione { $generation }: { $reason }

journal.cli.doctor.executable.directory = non è stato possibile determinare la directory dell'eseguibile: { $path }

journal.cli.doctor.execution.error = esecuzione del controllo non riuscita: { $kind }

journal.cli.doctor.execution.error.detail = esecuzione del controllo non riuscita: { $kind }: { $error }

journal.cli.doctor.facet.fix = { $unrouted_quantity ->
    [one] quel segmento manca dagli elenchi delle attività; controlla le dichiarazioni delle facette con `{ $doctor_command }` e le esecuzioni sense recenti con `{ $logs_command }`
    [many] quei segmenti mancano dagli elenchi delle attività; controlla le dichiarazioni delle facette con `{ $doctor_command }` e le esecuzioni sense recenti con `{ $logs_command }`
   *[other] quei segmenti mancano dagli elenchi delle attività; controlla le dichiarazioni delle facette con `{ $doctor_command }` e le esecuzioni sense recenti con `{ $logs_command }`
}

journal.cli.doctor.facet.no.active = nessun segmento attivo nei giorni più recenti

journal.cli.doctor.facet.routed = { $active_quantity ->
    [one] { $routed } di { $active } segmento attivo dal { $since } archiviato sotto una facetta
    [many] { $routed } di { $active } di segmenti attivi dal { $since } archiviati sotto una facetta
   *[other] { $routed } di { $active } segmenti attivi dal { $since } archiviati sotto una facetta
}

journal.cli.doctor.facet.unrouted = { $active_quantity ->
    [one] { $unrouted } di { $active } segmento attivo dal { $since } { $unrouted_quantity ->
        [one] è stato archiviato sotto nessuna facetta
        [many] sono stati archiviati sotto nessuna facetta
       *[other] sono stati archiviati sotto nessuna facetta
    }
    [many] { $unrouted } di { $active } di segmenti attivi dal { $since } { $unrouted_quantity ->
        [one] è stato archiviato sotto nessuna facetta
        [many] sono stati archiviati sotto nessuna facetta
       *[other] sono stati archiviati sotto nessuna facetta
    }
   *[other] { $unrouted } di { $active } segmenti attivi dal { $since } { $unrouted_quantity ->
        [one] è stato archiviato sotto nessuna facetta
        [many] sono stati archiviati sotto nessuna facetta
       *[other] sono stati archiviati sotto nessuna facetta
    }
}

journal.cli.doctor.help = uso: { $syntax }{ $line_break }{ $line_break }Esegui la diagnostica di { $brand }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $indent }{ $help_option }{ $help_gap }mostra questo messaggio di aiuto ed esci{ $line_break }{ $indent }{ $verbose_option }{ $verbose_gap }stampa ogni risultato di controllo{ $line_break }{ $indent }{ $json_option }{ $json_gap }emetti JSON invece del testo{ $line_break }{ $indent }{ $jsonl_option }{ $jsonl_gap }emetti eventi con un JSON per riga invece del testo{ $line_break }{ $indent }{ $port_option }{ $port_gap }porta da sondare (predefinita: { $port }){ $line_break }{ $indent }{ $readiness_option }{ $readiness_gap }esegui la batteria di prontezza della configurazione iniziale{ $line_break }{ $line_break }Se '{ $command }' non è disponibile, compila il doctor nativo da questo checkout:{ $line_break }{ $build_command }

journal.cli.doctor.holds.partition = partizione trattenuta { $partition }

journal.cli.doctor.holds.partition.stopped = la partizione { $partition } era in esecuzione quando il diario si è fermato; il diario la trattiene al prossimo avvio finché i suoi processi non sono spariti

journal.cli.doctor.holds.record.empty = il file di registro è vuoto

journal.cli.doctor.holds.record.filename = nome di file di partizione non valido nella directory dell'ambito; il diario lo legge comunque come registro di compito

journal.cli.doctor.holds.record.invalid = non è possibile analizzare il registro: { $error }

journal.cli.doctor.holds.record.stray = elemento che non è un registro nella directory dell'ambito

journal.cli.doctor.holds.record.unreadable = non è possibile leggere il registro: { $error }

journal.cli.doctor.holds.remove = rimuovi { $path }: non trattiene niente

journal.cli.doctor.holds.scope.unknown = formato della directory dell'ambito non riconosciuto; il diario trattiene comunque ogni compito registrato lì

journal.cli.doctor.holds.scope.unreadable = non è possibile elencare la directory dell'ambito: { $error }

journal.cli.doctor.holds.stray = estraneo che non è una directory nel percorso in corso della coda dei compiti

journal.cli.doctor.holds.unreadable = non è possibile elencare la directory dei trattenuti in corso: { $error }

journal.cli.doctor.index.fix.checking = la ricerca sta controllando questo da sé e ricostruisce se il danno è confermato; esegui { $command } per i dettagli

journal.cli.doctor.index.fix.deletions = la ricerca continua a riprovare queste rimozioni; esegui { $command } per i dettagli

journal.cli.doctor.index.fix.errors = esegui { $command } e controlla i suoi errori

journal.cli.doctor.index.fix.failed.files = la ricerca riprova questi aggiornamenti al suo controllo giornaliero; esegui { $command } per i dettagli

journal.cli.doctor.index.fix.failed.work = la ricerca riprova questo lavoro ogni ora; esegui { $command } per i dettagli

journal.cli.doctor.index.fix.full = esegui { $command }

journal.cli.doctor.index.fix.rebuild = la ricerca ricostruisce il suo indice dal tuo diario una volta confermato il danno; se persiste, esegui { $command }

journal.cli.doctor.index.fix.reset = esegui { $command }

journal.cli.doctor.index.fix.upgrade = aggiorna { $brand } alla versione più recente

journal.cli.doctor.index.fix.warnings = esegui { $command } e controlla i suoi avvisi

journal.cli.doctor.journal.absent = nessun diario in locale

journal.cli.doctor.journal.caught.up.backlog.unavailable = non è stato possibile determinare del tutto; non è stato possibile leggere l'arretrato: { $error }

journal.cli.doctor.journal.caught.up.chronicle.entry.unavailable = non è stato possibile determinare del tutto; non è stato possibile leggere l'elemento della cronaca: { $error }

journal.cli.doctor.journal.caught.up.chronicle.unavailable = non è stato possibile determinare del tutto; non è stato possibile leggere la cronaca: { $error }

journal.cli.doctor.journal.caught.up.configs.unavailable = non è stato possibile determinare del tutto; non è stato possibile leggere le configurazioni giornaliere: { $error }

journal.cli.doctor.journal.caught.up.coverage.unavailable = non è stato possibile determinare la copertura delle unità giornaliere il { $day }: { $error }

journal.cli.doctor.journal.caught.up.definitions.unavailable = non è stato possibile determinare del tutto; non è stato possibile individuare le definizioni dell'elaborazione giornaliera: { $error }

journal.cli.doctor.journal.caught.up.facets.day.unavailable = non è stato possibile determinare del tutto; non è stato possibile controllare le facette attive per { $day }: { $error }

journal.cli.doctor.journal.caught.up.facets.unavailable = non è stato possibile determinare del tutto; non è stato possibile leggere le facette: { $error }

journal.cli.doctor.journal.caught.up.inspect = apri lo stato per vedere il lavoro in attesa e ogni conferma di spesa; rielabora un giorno passato per dargli priorità dopo oggi

journal.cli.doctor.journal.caught.up.inventory.fix = apri lo stato per vedere il lavoro in attesa e il suo passo successivo

journal.cli.doctor.journal.caught.up.inventory.unavailable = ispezione dell'intero diario non disponibile: { $error }

journal.cli.doctor.journal.caught.up.inventory.waiting = l'ispezione dell'intero diario non ha ancora riportato

journal.cli.doctor.journal.caught.up.outside.backlog = unità giornaliera fuori dall'arretrato recente il { $day }

journal.cli.doctor.journal.caught.up.repair = esegui di nuovo { $doctor_command }; controlla i registri dello stato se persiste

journal.cli.doctor.journal.caught.up.unit.identity = non è stato possibile leggere il file dell'unità giornaliera { $file_name } il giorno { $day }: l'identità del registro non corrisponde al suo percorso

journal.cli.doctor.journal.caught.up.unit.invalid = non è stato possibile determinare del tutto; file dell'unità giornaliera malformato { $file_name } il giorno { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.unreadable = non è stato possibile determinare del tutto; file dell'unità giornaliera illeggibile { $file_name } il giorno { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.version = non è stato possibile leggere il file dell'unità giornaliera { $file_name } il giorno { $day }: versione del registro non supportata { $version }

journal.cli.doctor.journal.caught.up.units.entry.unavailable = non è stato possibile determinare del tutto; non è stato possibile leggere l'elemento dell'unità giornaliera per { $day }: { $error }

journal.cli.doctor.journal.caught.up.units.unavailable = non è stato possibile determinare del tutto; non è stato possibile leggere la directory delle unità giornaliere per { $day }: { $error }

journal.cli.doctor.journal.dir.writable.move = sposta o rimuovi { $path }, poi esegui di nuovo

journal.cli.doctor.journal.dir.writable.not.directory = il percorso del diario esiste ma non è una directory: { $path }

journal.cli.doctor.journal.dir.writable.parent.unwritable = directory del diario assente; l'antenato esistente più vicino non è scrivibile: { $path }

journal.cli.doctor.journal.dir.writable.parent.writable = directory del diario assente; la directory superiore { $path } è scrivibile

journal.cli.doctor.journal.dir.writable.unwritable = directory del diario non scrivibile: { $path }

journal.cli.doctor.journal.dir.writable.writable = directory del diario scrivibile: { $path }

journal.cli.doctor.journal.sources.readable.healthy = l'ultima scansione non ha trovato giorni illeggibili

journal.cli.doctor.journal.sources.readable.inspect.folders = controlla le cartelle dei giorni indicate sopra — un nome che non è una data del calendario non è un giorno che il diario può elaborare

journal.cli.doctor.journal.sources.readable.repair.files = ripara il file indicato sopra, poi esegui { $reprocess_command } per ogni giorno elencato che è già terminato

journal.cli.doctor.journal.sources.readable.reprocess = esegui { $reprocess_command } per ogni giorno elencato che è già terminato

journal.cli.doctor.journal.sources.readable.stats.invalid = non è stato possibile controllare — le statistiche non sono leggibili

journal.cli.doctor.journal.sources.readable.stats.missing = non è stato possibile controllare — nessuna statistica da leggere

journal.cli.doctor.journal.sources.readable.stats.predates = non è stato possibile controllare — le statistiche precedono questa versione

journal.cli.doctor.launchd.stale.plist.absent = plist di launchd assente

journal.cli.doctor.launchd.stale.plist.arguments.missing = al plist manca { $plist_argument }

journal.cli.doctor.launchd.stale.plist.invalid = non è stato possibile analizzare il plist: { $plist_error }: { $error }

journal.cli.doctor.launchd.stale.plist.repair = esegui { $uninstall_command }, poi esegui { $install_command } separatamente per reinstallare un servizio headless in secondo piano

journal.cli.doctor.launchd.stale.plist.target.exists = la destinazione del plist di launchd esiste ({ $path })

journal.cli.doctor.launchd.stale.plist.target.missing = il plist punta a un eseguibile assente: { $path }

journal.cli.doctor.local.bin.solstone.reachable.file.invalid = { $path } non è un file

journal.cli.doctor.local.bin.solstone.reachable.file.missing = { $path } è assente

journal.cli.doctor.local.bin.solstone.reachable.path.absent = { $brand } non è su { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.path.mismatch = { $path_variable } { $brand } si risolve in { $resolved }, atteso { $expected }

journal.cli.doctor.local.bin.solstone.reachable.path.ready = { $wrapper_path } è su { $path_variable } in { $path }

journal.cli.doctor.local.bin.solstone.reachable.path.repair = metti { $bin_dir } prima su { $path_variable }, o esegui { $setup_command } per reindirizzare l'involucro gestito

journal.cli.doctor.local.bin.solstone.reachable.symlink.ready = { $wrapper_path } è un link simbolico a { $path_variable } { $brand } in { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.path.priority = metti { $install_dir } prima di { $other_dir } sul tuo { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.windows.ready = { $brand } è su { $path_variable } in { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.repair = reinstalla il diario per ripristinare { $executable }

journal.cli.doctor.local.bin.solstone.reachable.windows.terminal = apri una nuova finestra del terminale e riprova; se { $brand } non si trova ancora, reinstalla il diario

journal.cli.doctor.local.bin.solstone.reachable.wrapper.install = esegui { $setup_command } per installare l'involucro gestito di { $brand }

journal.cli.doctor.orphan.pdf.found = { $quantity ->
    [one] { $count } originale PDF grezzo senza una trascrizione del documento leggibile: { $paths }
    [many] { $count } di originali PDF grezzi senza una trascrizione del documento leggibile: { $paths }
   *[other] { $count } originali PDF grezzi senza una trascrizione del documento leggibile: { $paths }
}

journal.cli.doctor.orphan.segment.pdf.chronicle.unavailable = directory della cronaca non disponibile

journal.cli.doctor.orphan.segment.pdf.clear = tutti gli originali PDF grezzi hanno trascrizioni del documento leggibili

journal.cli.doctor.orphan.segment.pdf.repair = aggiungi un { $transcript_file } leggibile accanto all'originale PDF, poi esegui di nuovo { $doctor_command }

journal.cli.doctor.package.executable.parent = l'eseguibile non ha una directory superiore

journal.cli.doctor.package.layout = non è stato possibile individuare le radici dei talenti del pacchetto dalla directory dell'eseguibile { $directory }{ $newline }antenato attraversato { $ancestor }{ $newline }candidato site-packages: { $site }{ $newline }candidato di checkout ({ $pyproject } + { $git_dir }): { $checkout }{ $newline }ancore share della disposizione { $bundle }, { $layout }, { $template }: { $install }{ $newline }candidato del bundle Apple { $journal_payload }: { $embedded }

journal.cli.doctor.package.missing = alla radice dell'installazione { $root } manca la directory { $path }

journal.cli.doctor.package.none = nessuno

journal.cli.doctor.package.target = { $newline }la directory target di Cargo { $target } è fuori da ogni checkout: un binario compilato lì non può trovare il payload, quindi tieni { $target_env } dentro il checkout

journal.cli.doctor.package.unavailable = non è stato possibile individuare le radici dei talenti del pacchetto dalla directory dell'eseguibile { $directory }

journal.cli.doctor.parakeet.coreml.configured = controllo di parakeet non riuscito: la cache installata { $installed } non corrisponde a quella configurata { $configured }

journal.cli.doctor.parakeet.coreml.digest = controllo di parakeet non riuscito: hash della risorsa non corrispondente per { $file }: atteso { $expected }, trovato { $found }

journal.cli.doctor.parakeet.coreml.failed = controllo di parakeet non riuscito: { $error }

journal.cli.doctor.parakeet.coreml.missing = controllo di parakeet non riuscito: risorsa assente: { $file }

journal.cli.doctor.parakeet.coreml.missing.error = controllo di parakeet non riuscito: risorsa assente: { $file }: { $error }

journal.cli.doctor.parakeet.coreml.sentinel = controllo di parakeet non riuscito: sentinella non pronta in { $path }

journal.cli.doctor.parakeet.coreml.size = controllo di parakeet non riuscito: dimensione della risorsa non corrispondente per { $file }: atteso { $expected }, trovato { $found }

journal.cli.doctor.parakeet.coreml.unreadable = controllo di parakeet non riuscito: non è stato possibile leggere { $file }: { $error }

journal.cli.doctor.parakeet.cpp.executable = controllo di { $parakeet_cpp } non riuscito: { $name } non eseguibile in { $path }

journal.cli.doctor.parakeet.cpp.missing = controllo di { $parakeet_cpp } non riuscito: { $name } assente in { $path }

journal.cli.doctor.parakeet.cpp.model = controllo di { $parakeet_cpp } non riuscito: modello assente in { $path }

journal.cli.doctor.parakeet.cpp.pin = al pin di { $parakeet_cpp } manca { $key }

journal.cli.doctor.parakeet.cpp.platform = { $parakeet_cpp } non è supportato su { $os }/{ $arch }

journal.cli.doctor.parakeet.cpp.stt.ready.backend.not.selected = il backend configurato non è { $backend_name }; controllo non applicabile

journal.cli.doctor.parakeet.cpp.stt.ready.binary.unavailable = il binario di { $backend_name } non può avviarsi

journal.cli.doctor.parakeet.cpp.stt.ready.install = gli artefatti di { $backend_name } non sono installati — recuperali con: { $provider_command }

journal.cli.doctor.parakeet.cpp.stt.ready.offline = { $server } non raggiungibile: il tuo diario non è in esecuzione

journal.cli.doctor.parakeet.cpp.stt.ready.online = { $backend_name } pronto (server e modello corrispondono al pacchetto firmato, server raggiungibile)

journal.cli.doctor.parakeet.cpp.stt.ready.ready = { $backend_name } pronto (binari e modello installati, server raggiungibile)

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.install = installa l'ambiente di esecuzione di sistema { $openmp } che fornisce { $runtime_file }, poi esegui di nuovo { $doctor_command }

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.unavailable = { $backend_name } non può avviarsi: ambiente di esecuzione { $openmp } non disponibile ({ $runtime_file })

journal.cli.doctor.parakeet.cpp.stt.ready.server.missing = { $server } non raggiungibile: il tuo diario è in esecuzione, ma non { $server }

journal.cli.doctor.parakeet.cpp.stt.ready.start = { $server } non è raggiungibile — avvia il servizio del diario: { $start_command }

journal.cli.doctor.parakeet.cpp.stt.ready.unreachable = { $server } non raggiungibile: { $error }

journal.cli.doctor.parakeet.cpp.stt.ready.unsupported = { $backend_name } è supportato solo su linux e windows

journal.cli.doctor.parakeet.phase = { $server } non raggiungibile: il tuo diario riporta il suo stato come { $phase }

journal.cli.doctor.parakeet.phase.checking = { $server } non raggiungibile: il tuo diario riporta il suo stato come controllo

journal.cli.doctor.parakeet.reinstall = reinstalla il diario

journal.cli.doctor.parakeet.unreachable = { $cause ->
    [absent] { $server } non raggiungibile: non c'è niente in ascolto
    [unknown] { $server } non raggiungibile: non è possibile capire se qualcosa è in ascolto
    [runtime] { $server } non raggiungibile: niente ha risposto
    [timeout] { $server } non raggiungibile: ha impiegato troppo tempo a rispondere
   *[transport] { $server } non raggiungibile: la connessione si è interrotta
}

journal.cli.doctor.path.repair = correggi proprietà/permessi di { $path }

journal.cli.doctor.platform.unsupported = non supportato su { $platform }

journal.cli.doctor.progress.check = { $seconds } s controlla { $name }

journal.cli.doctor.progress.hash = { $seconds } s verifica { $member }: { $hashed } di { $total } { $megabytes }

journal.cli.doctor.search.log.finished = non è stato possibile leggere l'ultimo controllo di ricerca terminato: { $error }

journal.cli.doctor.search.log.unavailable = stato dell'indice di ricerca non disponibile: { $error }

journal.cli.doctor.search.log.work = non è stato possibile leggere l'elenco dei lavori di ricerca: { $error }

journal.cli.doctor.sense.dispatch.capped.reason = almeno 99 errori di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo: { $reason }

journal.cli.doctor.sense.dispatch.capped.unknown = almeno 99 errori di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo non disponibile

journal.cli.doctor.sense.dispatch.clear = nessun errore di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita

journal.cli.doctor.sense.dispatch.count.invalid = lo stato di Sense è incompleto: nessun conteggio valido non negativo degli errori di smistamento

journal.cli.doctor.sense.dispatch.count.out.of.range = lo stato di Sense è incompleto: il conteggio degli errori di smistamento è fuori dal suo intervallo supportato

journal.cli.doctor.sense.dispatch.errors.reason = { $quantity ->
    [one] { $count } errore di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo: { $reason }
    [many] { $count } di errori di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo: { $reason }
   *[other] { $count } errori di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo: { $reason }
}

journal.cli.doctor.sense.dispatch.errors.unknown = { $quantity ->
    [one] { $count } errore di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo non disponibile
    [many] { $count } di errori di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo non disponibile
   *[other] { $count } errori di smistamento di Sense dall'ultima sincronizzazione degli handler riuscita; ultimo motivo non disponibile
}

journal.cli.doctor.sense.dispatch.unreachable = { $cause ->
    [absent] non è stato possibile confermare lo stato dello smistamento di Sense: non c'è niente in ascolto
    [unknown] non è stato possibile confermare lo stato dello smistamento di Sense: non è possibile capire se qualcosa è in ascolto
    [runtime] non è stato possibile confermare lo stato dello smistamento di Sense: niente ha risposto
    [timeout] non è stato possibile confermare lo stato dello smistamento di Sense: ha impiegato troppo tempo a rispondere
   *[transport] non è stato possibile confermare lo stato dello smistamento di Sense: la connessione si è interrotta
}

journal.cli.doctor.service.absent = nessun servizio del diario in locale

journal.cli.doctor.service.conflict.warning = risolvi l'avviso sulla topologia del supervisore di macOS prima di modificare il servizio del diario

journal.cli.doctor.service.crash.list = ciclo di arresti anomali: { $services }

journal.cli.doctor.service.identity.repair = esegui { $setup_command } per reinstallare il servizio

journal.cli.doctor.service.identity.repair.force = esegui { $setup_command } da questa installazione per aggiornare il servizio

journal.cli.doctor.service.identity.target.absent = l'unità del servizio è malformata: nessuna destinazione eseguibile in { $path }

journal.cli.doctor.service.identity.target.current = la destinazione del servizio corrisponde all'installazione attuale: { $raw } -> { $path }

journal.cli.doctor.service.identity.target.mismatch = destinazione del servizio non corrispondente: { $raw } si risolve in { $resolved }, atteso { $expected }

journal.cli.doctor.service.identity.windows.current = la destinazione del servizio corrisponde all'installazione attuale: { $command }

journal.cli.doctor.service.identity.windows.mismatch = destinazione del servizio non corrispondente: { $mismatch }

journal.cli.doctor.service.registration.unreadable = non è stato possibile leggere la registrazione del servizio del diario — { $reason }

journal.cli.doctor.service.restart.attempts = { $quantity ->
    [one] { $name } ({ $attempts } tentativo di riavvio)
    [many] { $name } ({ $attempts } di tentativi di riavvio)
   *[other] { $name } ({ $attempts } tentativi di riavvio)
}

journal.cli.doctor.service.running.logs = esegui { $logs_command }

journal.cli.doctor.service.running.pid.invalid = il PID del processo figlio di probe non entra in un ID di gruppo di processi

journal.cli.doctor.service.running.registration.no.reader = nessun lettore per la registrazione del servizio

journal.cli.doctor.service.running.restart = esegui { $restart_command }; se persiste, esegui { $logs_command }

journal.cli.doctor.service.running.running = il servizio del diario è in esecuzione

journal.cli.doctor.service.running.start = esegui { $start_command }

journal.cli.doctor.service.running.stopped = servizio installato ma non in esecuzione

journal.cli.doctor.service.running.unit.failed = l'unità del servizio del diario è fallita

journal.cli.doctor.service.running.unreachable = { $cause ->
    [absent] non è stato possibile ottenere uno stato — non c'è niente in ascolto
    [unknown] non è stato possibile ottenere uno stato — non è possibile capire se qualcosa è in ascolto
    [runtime] non è stato possibile ottenere uno stato — niente ha risposto
    [timeout] non è stato possibile ottenere uno stato — ha impiegato troppo tempo a rispondere
   *[transport] non è stato possibile ottenere uno stato — la connessione si è interrotta
}

journal.cli.doctor.skill.state.checkout.required = lo stato dello skill del router è disponibile solo da un checkout dei sorgenti

journal.cli.doctor.skill.state.directories.missing = le directory degli skill del router non sono disponibili

journal.cli.doctor.skill.state.install = esegui { $skills_command }

journal.cli.doctor.skill.state.link.foreign = { $skill } punta altrove in { $path }

journal.cli.doctor.skill.state.link.missing = { $skill } assente in { $path }

journal.cli.doctor.skill.state.link.stale = link dello skill del router non aggiornato in { $path }

journal.cli.doctor.skill.state.links.ready = gli skill del router { $brand }, journal sono installati e correnti

journal.cli.doctor.source.default = predefinita

journal.cli.doctor.sources.days.more = { $days }, e altri { $more }

journal.cli.doctor.sources.fold.failed = { $quantity ->
    [one] { $count } giorno non ha terminato l'elaborazione ({ $days })
    [many] { $count } di giorni non hanno terminato l'elaborazione ({ $days })
   *[other] { $count } giorni non hanno terminato l'elaborazione ({ $days })
}

journal.cli.doctor.sources.statistics.refresh = esegui { $command }; controlla i registri dello stato se persiste

journal.cli.doctor.sources.statistics.stale = non è stato possibile controllare — le statistiche sono vecchie di { $hours } ore

journal.cli.doctor.sources.unreadable = { $quantity ->
    [one] non è stato possibile scansionare { $count } giorno ({ $days })
    [many] non è stato possibile scansionare { $count } di giorni ({ $days })
   *[other] non è stato possibile scansionare { $count } giorni ({ $days })
}

journal.cli.doctor.sources.unreadable.cause = { $quantity ->
    [one] non è stato possibile scansionare { $count } giorno ({ $days }): { $day }: { $cause }
    [many] non è stato possibile scansionare { $count } di giorni ({ $days }): { $day }: { $cause }
   *[other] non è stato possibile scansionare { $count } giorni ({ $days }): { $day }: { $cause }
}

journal.cli.doctor.sources.unreadable.undated = { $quantity ->
    [one] non è stato possibile scansionare { $count } giorno ({ $days }): quel giorno: { $cause }
    [many] non è stato possibile scansionare { $count } di giorni ({ $days }): quel giorno: { $cause }
   *[other] non è stato possibile scansionare { $count } giorni ({ $days }): quel giorno: { $cause }
}

journal.cli.doctor.speakers.installation.app = l'installazione di { $speakers_name } è incompleta ({ $detail }). Ripara: reinstalla l'app del diario.

journal.cli.doctor.speakers.installation.journal = l'installazione di { $speakers_name } è incompleta ({ $detail }). Ripara: reinstalla il diario.

journal.cli.doctor.speakers.ready = installazione di { $command } pronta

journal.cli.doctor.speakers.reinstall.app = reinstalla l'app del diario

journal.cli.doctor.speakers.unavailable = installazione di { $command } non disponibile

journal.cli.doctor.status.error = ERROR

journal.cli.doctor.status.fail = FAIL

journal.cli.doctor.status.ok = OK

journal.cli.doctor.status.skip = SKIP

journal.cli.doctor.status.warn = WARN

journal.cli.doctor.storage.inspection.log = privacy dell'archiviazione: { $error }

journal.cli.doctor.storage.privacy.choice = altri account su questo computer potrebbero riuscire a leggere il tuo diario. non è stato possibile rendere privata la sua cartella su questa unità. puoi tenerlo su questa unità o spostarlo su un'unità che tiene private le cartelle. per tenerlo su questa unità, scegli quello nella pagina di stato del tuo diario.

journal.cli.doctor.summary = { $total_quantity ->
    [one] doctor: { $total } controllo, { $failed } non riusciti, { $warnings } { $warning_quantity ->
        [one] avviso
        [many] di avvisi
       *[other] avvisi
    }, { $skipped } saltati, { $errors } { $error_quantity ->
        [one] errore
        [many] di errori
       *[other] errori
    }
    [many] doctor: { $total } di controlli, { $failed } non riusciti, { $warnings } { $warning_quantity ->
        [one] avviso
        [many] di avvisi
       *[other] avvisi
    }, { $skipped } saltati, { $errors } { $error_quantity ->
        [one] errore
        [many] di errori
       *[other] errori
    }
   *[other] doctor: { $total } controlli, { $failed } non riusciti, { $warnings } { $warning_quantity ->
        [one] avviso
        [many] di avvisi
       *[other] avvisi
    }, { $skipped } saltati, { $errors } { $error_quantity ->
        [one] errore
        [many] di errori
       *[other] errori
    }
}

journal.cli.doctor.supervisor.conflict.clear = nessun conflitto del supervisore di macOS (controllo nativo: solo scansione di programmi di avvio estranei; nessun programma di avvio KeepAlive concorrente trovato)

journal.cli.doctor.supervisor.conflict.repair = { $quantity ->
    [one] rimuovi il programma di avvio estraneo che punta a { $app_path }: { $commands }; poi esegui di nuovo { $doctor_command }
    [many] rimuovi i programmi di avvio estranei che puntano a { $app_path }: { $commands }; poi esegui di nuovo { $doctor_command }
   *[other] rimuovi i programmi di avvio estranei che puntano a { $app_path }: { $commands }; poi esegui di nuovo { $doctor_command }
}

journal.cli.doctor.supervisor.conflict.report = { $quantity ->
    [one] conflitto del supervisore di macOS: { $count } programma di avvio { $keep_alive } estraneo punta a { $app_path }
    [many] conflitto del supervisore di macOS: { $count } di programmi di avvio { $keep_alive } estranei puntano a { $app_path }
   *[other] conflitto del supervisore di macOS: { $count } programmi di avvio { $keep_alive } estranei puntano a { $app_path }
}

journal.cli.doctor.supervisor.conflict.resolve = { $quantity ->
    [one] risolvi prima il conflitto del supervisore di macOS: rimuovi il programma di avvio estraneo che punta a { $app_path }: { $commands }; poi esegui di nuovo { $doctor_command }
    [many] risolvi prima il conflitto del supervisore di macOS: rimuovi i programmi di avvio estranei che puntano a { $app_path }: { $commands }; poi esegui di nuovo { $doctor_command }
   *[other] risolvi prima il conflitto del supervisore di macOS: rimuovi i programmi di avvio estranei che puntano a { $app_path }: { $commands }; poi esegui di nuovo { $doctor_command }
}

journal.cli.doctor.supervisor.conflict.unsupported = non supportato su windows

journal.cli.doctor.sync.directory.required = un discendente della directory piatta non è una directory: { $path }

journal.cli.doctor.sync.entry.changed = elemento cambiato durante l'osservazione: { $reason }

journal.cli.doctor.sync.entry.invalid = elemento della directory piatta non valido { $entry }: { $reason }

journal.cli.doctor.sync.entry.replaced = cambiato o scomparso durante l'ispezione

journal.cli.doctor.sync.entry.unsafe = Installazione: richiede attenzione{ $newline }il tuo diario contiene un elemento che non è possibile controllare in sicurezza.{ $newline }sposta l'elemento indicato sotto dettagli fuori dalla cartella `{ $sync_folder }` del tuo diario, poi riprova.{ $newline }{ $newline }dettagli:{ $newline }percorso: { $path }{ $newline }tipo: { $entry_type }{ $newline }motivo: { $reason }

journal.cli.doctor.sync.entry.unsafe.bytes = Installazione: richiede attenzione{ $newline }il tuo diario contiene un elemento che non è possibile controllare in sicurezza.{ $newline }sposta l'elemento indicato sotto dettagli fuori dalla cartella `{ $sync_folder }` del tuo diario, poi riprova.{ $newline }{ $newline }dettagli:{ $newline }percorso: { $path }{ $newline }tipo: { $entry_type }{ $newline }motivo: { $reason }{ $newline }byte del nome: { $bytes }

journal.cli.doctor.sync.entry.vanished = elemento della directory piatta scomparso durante l'elencazione: { $path }

journal.cli.doctor.sync.file.required = l'elemento della directory piatta non è un file regolare: { $path }

journal.cli.doctor.sync.folder.full = Installazione: richiede attenzione{ $newline }il tuo diario ha troppi elementi per controllarli in sicurezza.{ $newline }rivedi gli elementi nella cartella `{ $sync_folder }` del tuo diario. sposta solo gli elementi che sai non appartenere lì finché ne restano 256 o meno, poi riprova.{ $newline }{ $newline }dettagli:{ $newline }cartella: { $folder }{ $newline }elementi trovati: più di 256{ $newline }massimo controllato: 256

journal.cli.doctor.sync.folder.unsafe = Installazione: richiede attenzione{ $newline }non è possibile controllare in sicurezza una parte del tuo diario.{ $newline }controlla il percorso indicato sotto dettagli. deve essere una cartella ordinaria dentro il tuo diario che puoi aprire prima di riprovare.{ $newline }{ $newline }dettagli:{ $newline }percorso: { $path }{ $newline }motivo: { $reason }

journal.cli.doctor.sync.heartbeat = Installazione: richiede attenzione{ $newline }è presente un battito recente di un'altra esecuzione.{ $newline }aspetta un momento, poi riprova.

journal.cli.doctor.sync.identity.changed = identità della directory piatta cambiata: { $path }

journal.cli.doctor.sync.io = { $operation } non riuscito per { $path }: { $reason }

journal.cli.doctor.sync.kind.blockdevice = dispositivo a blocchi

journal.cli.doctor.sync.kind.characterdevice = dispositivo a caratteri

journal.cli.doctor.sync.kind.directory = directory

journal.cli.doctor.sync.kind.fifo = pipe con nome

journal.cli.doctor.sync.kind.other = altro oggetto del filesystem

journal.cli.doctor.sync.kind.regularfile = file regolare

journal.cli.doctor.sync.kind.socket = socket Unix

journal.cli.doctor.sync.kind.symlink = link simbolico

journal.cli.doctor.sync.local = solo questo computer ({ $host })

journal.cli.doctor.sync.path.invalid = percorso della directory piatta non valido { $path }: { $reason }

journal.cli.doctor.sync.peer.legacy = solo questo computer ({ $host }){ $newline }  ultimo altro scrittore: { $writer } (macchina precedente { $prefix }...)

journal.cli.doctor.sync.peer.unknown = solo questo computer ({ $host }){ $newline }  ultimo altro scrittore: { $writer } (battito non identificato)

journal.cli.doctor.sync.peer.writer = solo questo computer ({ $host }){ $newline }  ultimo altro scrittore: { $writer } (scrittore { $prefix }... esecuzione { $run })

journal.cli.doctor.sync.reason.disappeared = elemento scomparso durante l'osservazione

journal.cli.doctor.sync.reason.nonregular = non è un file regolare ({ $kind })

journal.cli.doctor.sync.reason.oversized = il file regolare è di { $size } byte, il limite è { $limit }

journal.cli.doctor.sync.reason.read = non è stato possibile leggere il file osservato

journal.cli.doctor.sync.size = l'elemento della directory piatta supera il limite di lettura osservata: { $path } è { $kind }, { $size } byte superano { $limit }

journal.cli.doctor.sync.symlink = un discendente della directory piatta è un link simbolico: { $path }

journal.cli.doctor.sync.unsupported = Installazione: richiede attenzione{ $newline }la capacità di riscansionare la sincronizzazione non è disponibile su questa piattaforma.

journal.cli.doctor.sync.unverifiable = Installazione: richiede attenzione{ $newline }non è stato possibile verificare lo stato di avvio.{ $newline }aspetta un momento, poi riprova.

journal.cli.doctor.sync.waiting = Installazione: in attesa{ $newline }è stato trovato un battito recente di un'altra esecuzione.{ $newline }l'app di { $brand } è in attesa mentre controlla se quell'attività continua. controllerà di nuovo a breve.

journal.cli.doctor.task.pace.about.cap = un lavoro è in esecuzione da molto; verrà fermato automaticamente se supera il suo cap — nessuna azione necessaria, a meno che non persista

journal.cli.doctor.task.pace.healthy = compiti al passo

journal.cli.doctor.task.pace.status.unavailable = stato del supervisore non disponibile

journal.cli.doctor.task.queue.holds.absent = nessun registro di partizione di compiti in corso o trattenuti presente

journal.cli.doctor.task.queue.holds.clear = tutti i registri di partizione dei compiti sono puliti

journal.cli.doctor.tasks.duration = { $task } ({ $seconds } s di { $limit } s di cap)

journal.cli.doctor.tasks.long = in esecuzione da molto: { $tasks }

journal.cli.doctor.transcription.unretryable = { $quantity ->
    [one] { $count } registrazione che il diario non riproverà da solo ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
    [many] { $count } di registrazioni che il diario non riproverà da solo ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
   *[other] { $count } registrazioni che il diario non riproverà da solo ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
}

journal.cli.doctor.unretryable.transcribe.input.clear = 0 registrazioni che il diario non riproverà da solo (handler: { $handler }, { $reason_key }: { $reason_code })

journal.cli.doctor.unretryable.transcribe.input.days.unreadable = non è stato possibile leggere i giorni della cronaca: { $error }

journal.cli.doctor.unretryable.transcribe.input.directory.unreadable = non è possibile leggere la directory del segmento { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.entry.unreadable = non è possibile leggere l'elemento del segmento in { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.handler.missing = al registro di elaborazione in { $path } manca il campo handler

journal.cli.doctor.unretryable.transcribe.input.identity.invalid = identità del segmento non valida in { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.open = non è possibile aprire { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.read = non è possibile leggere { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.encoding = la prima riga in { $path } non è utf-8 valido: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.json = la prima riga in { $path } non è json valido: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.large = la prima riga in { $path } supera il limite di byte

journal.cli.doctor.unretryable.transcribe.input.row.unavailable = la prima riga in { $path } non può essere stabilita o supera il limite di byte

journal.cli.doctor.unretryable.transcribe.input.scan.failed = non è stato possibile determinare quante registrazioni il diario non riproverà da solo ({ $reason })

journal.cli.doctor.unretryable.transcribe.input.segments.unreadable = non è stato possibile elencare i segmenti per il giorno { $day }: { $error }

journal.cli.doctor.usage.argument = uso: { $syntax }{ $line_break }{ $command }: errore: argomento non atteso

journal.cli.doctor.usage.formats = uso: { $syntax }{ $line_break }{ $command }: errore: { $json_flag } e { $jsonl_flag } sono mutuamente esclusivi

journal.cli.doctor.usage.port = uso: { $syntax }{ $line_break }{ $command }: errore: { $port_flag } richiede un intero

journal.cli.doctor.vad.contract = l'ausiliario { $vad } eseguito ma non ha riportato il contratto d'uso con stdin chiuso (uscita { $exit_code }): { $stderr }

journal.cli.doctor.vad.executable = non è stato possibile determinare l'eseguibile attuale dell'ausiliario { $vad }: { $error }

journal.cli.doctor.vad.layout = la ricerca dell'ausiliario { $vad } è fuori dal payload dell'app firmata in esecuzione

journal.cli.doctor.vad.loader = l'ausiliario { $vad } non è riuscito a caricare { $library }

journal.cli.doctor.vad.missing = manca il binario dell'ausiliario { $vad }: { $path }

journal.cli.doctor.vad.parent = l'eseguibile attuale non ha una directory superiore per la ricerca dell'ausiliario { $vad }

journal.cli.doctor.vad.probe.log = sonda dell'ausiliario { $vad } non riuscita: { $error }

journal.cli.doctor.vad.ready = ausiliario { $vad } avviabile (contratto d'uso con stdin chiuso)

journal.cli.doctor.vad.repair.install = reinstalla il diario, poi esegui di nuovo { $doctor_command }

journal.cli.doctor.vad.repair.install.app = reinstalla l'app del diario, poi esegui di nuovo { $doctor_command }

journal.cli.doctor.vad.repair.restart = riavvia il diario, poi esegui di nuovo { $doctor_command }; se scade ancora il tempo, reinstalla il diario

journal.cli.doctor.vad.repair.restart.app = riavvia l'app del diario, poi esegui di nuovo { $doctor_command }; se scade ancora il tempo, reinstalla l'app del diario

journal.cli.doctor.vad.spawn = non è stato possibile eseguire l'ausiliario { $vad }: { $cause }

journal.cli.doctor.vad.timeout = l'ausiliario { $vad } non è uscito entro { $timeout }

journal.cli.doctor.vad.unresolved = non è stato possibile risolvere il percorso dell'ausiliario { $vad }: { $cause }

journal.cli.doctor.windows.onnx.helper = l'ausiliario { $onnx } non è il membro dichiarato del payload dell'app firmata

journal.cli.doctor.windows.onnx.member = il payload dell'app { $onnx } firmata rifiuta { $member }: { $error }

journal.cli.doctor.windows.onnx.platform = la verifica del pacchetto { $windows } { $onnx } richiede un ambiente di esecuzione { $windows }

journal.cli.doctor.windows.onnx.verification = non è stato possibile verificare il payload dell'app { $onnx } firmata: { $error }

journal.cli.doctor.windows.parakeet.executable = non è stato possibile determinare l'eseguibile del diario in esecuzione: { $error }

journal.cli.doctor.windows.parakeet.layout = l'eseguibile del diario in esecuzione non è nella directory bin del pacchetto: { $path }

journal.cli.doctor.windows.parakeet.member = il payload dell'app { $parakeet_name } firmata rifiuta { $member }: { $error }

journal.cli.doctor.windows.parakeet.parent = l'eseguibile del diario in esecuzione non ha una directory superiore: { $path }

journal.cli.doctor.windows.parakeet.platform = La verifica del pacchetto Windows { $parakeet_name } richiede un ambiente di esecuzione Windows

journal.cli.doctor.windows.parakeet.root = la directory bin del pacchetto non ha una radice del pacchetto: { $path }

journal.cli.doctor.windows.parakeet.verification = non è stato possibile verificare il payload dell'app { $parakeet_name } firmata: { $error }

journal.cli.host.arguments.invalid = { $command }: errore: argomenti non validi

journal.cli.host.backfill.facet.ids.help = uso: { $syntax }{ $line_break }{ $line_break }Completa gli identificatori { $term } mancanti in tutte le dichiarazioni di facette.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_4 }Scrivi su disco gli ID di facetta completati (predefinito: prova a vuoto){ $line_break }

journal.cli.host.backfill.facet.ids.solstone.journal.backfill.facet.ids.error = { $command_solstone_journal_backfill_facet_ids }: errore: { $error }

journal.cli.host.backfill.facet.ids.usage = uso: { $syntax }{ $line_break }

journal.cli.host.backfill.facets.applied = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Esaminata { $scanned } facetta; completato un ID per { $backfilled } facetta.
        [many] Esaminata { $scanned } facetta; completati ID per { $backfilled } di facette.
       *[other] Esaminata { $scanned } facetta; completati ID per { $backfilled } facette.
        }
    [many] { $backfilled_quantity ->
        [one] Esaminate { $scanned } di facette; completato un ID per { $backfilled } facetta.
        [many] Esaminate { $scanned } di facette; completati ID per { $backfilled } di facette.
       *[other] Esaminate { $scanned } di facette; completati ID per { $backfilled } facette.
        }
   *[other] { $backfilled_quantity ->
        [one] Esaminate { $scanned } facette; completato un ID per { $backfilled } facetta.
        [many] Esaminate { $scanned } facette; completati ID per { $backfilled } di facette.
       *[other] Esaminate { $scanned } facette; completati ID per { $backfilled } facette.
        }
}

journal.cli.host.backfill.facets.dry = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Prova a vuoto: esaminata { $scanned } facetta; { $backfilled } facetta richiede un ID (passa { $literal_commit } per applicare).
        [many] Prova a vuoto: esaminata { $scanned } facetta; { $backfilled } di facette richiedono ID (passa { $literal_commit } per applicare).
       *[other] Prova a vuoto: esaminata { $scanned } facetta; { $backfilled } facette richiedono ID (passa { $literal_commit } per applicare).
        }
    [many] { $backfilled_quantity ->
        [one] Prova a vuoto: esaminate { $scanned } di facette; { $backfilled } facetta richiede un ID (passa { $literal_commit } per applicare).
        [many] Prova a vuoto: esaminate { $scanned } di facette; { $backfilled } di facette richiedono ID (passa { $literal_commit } per applicare).
       *[other] Prova a vuoto: esaminate { $scanned } di facette; { $backfilled } facette richiedono ID (passa { $literal_commit } per applicare).
        }
   *[other] { $backfilled_quantity ->
        [one] Prova a vuoto: esaminate { $scanned } facette; { $backfilled } facetta richiede un ID (passa { $literal_commit } per applicare).
        [many] Prova a vuoto: esaminate { $scanned } facette; { $backfilled } di facette richiedono ID (passa { $literal_commit } per applicare).
       *[other] Prova a vuoto: esaminate { $scanned } facette; { $backfilled } facette richiedono ID (passa { $literal_commit } per applicare).
        }
}

journal.cli.host.check.help = uso: { $syntax }{ $line_break }{ $line_break }Verdetto di prontezza per i modelli locali del diario inclusi.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }emetti il verdetto di prontezza come { $term } per gli agenti{ $line_break }

journal.cli.host.check.usage = uso: { $syntax }{ $line_break }

journal.cli.host.config.admit.rewrite.identity.admission = solstone journal config: ammissione dell'identità rifiutata: { $error }

journal.cli.host.config.admit.rewrite.identity.owner = solstone journal config: titolare dell'identità rifiutato: { $error }

journal.cli.host.config.admit.rewrite.identity.root = solstone journal config: radice dell'identità rifiutata: { $error }

journal.cli.host.config.admit.rewrite.path = solstone journal config: percorso del diario rifiutato: { $error }

journal.cli.host.config.corrupt.settings.file.at.path.couldn = non è stato possibile leggere il tuo file delle impostazioni in { $path }. le tue impostazioni non sono state modificate. ripara il file o ripristina { $literal_1 } da un backup, poi riprova.

journal.cli.host.config.cross.filesystem.cannot.move.across.filesystems.filesystem = solstone journal config: rifiutato: non è stato possibile spostare tra filesystem (id dei filesystem { $source_device } e { $target_device }); ferma il tuo diario con '{ $literal_1 }', copia la cartella del diario nel nuovo percorso, usa { $literal_3 }, poi avvialo di nuovo con '{ $literal_2 }'

journal.cli.host.config.decide.move.requires.not.active.target = solstone journal config: rifiutato: { $literal_3 } richiede una destinazione non attiva; l'attuale è { $current_state } e la destinazione è { $target_state }; flag validi: { $literal_1 }, { $literal_2 }

journal.cli.host.config.execute.already.set.path = solstone journal config: diario già impostato su { $path }

journal.cli.host.config.execute.before.mutation = solstone journal config: rifiutato prima della mutazione: { $error }

journal.cli.host.config.execute.warning.force.bypasses.confirmation.and = solstone journal config: avviso: { $literal_1 } aggira la conferma e i controlli di attività della destinazione

journal.cli.host.config.existing.target.move.target.already.exists.path = solstone journal config: rifiutato: la destinazione dello spostamento esiste già: { $path }

journal.cli.host.config.help = uso: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_10 }mostra il percorso configurato del diario e la fonte{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_2 }cambia il percorso del diario usato da questa installazione{ $line_break }

journal.cli.host.config.identity.root.from.current.executable = solstone journal config: { $error }

journal.cli.host.config.journal.path.contains.shell.active.character = solstone journal config: rifiutato: il percorso del diario contiene un carattere attivo per la shell: { $path }

journal.cli.host.config.journal.path.source.tree.fallback.path = solstone journal config: rifiutato: { $path } è il percorso di ripiego dell'albero dei sorgenti ma questo non è un checkout dei sorgenti

journal.cli.host.config.maybe.restart.current.service.could.not.restart.service.at = non è stato possibile riavviare il servizio nella cartella originale del diario

journal.cli.host.config.missing.current.move.source.does.not.exist = solstone journal config: rifiutato: l'origine dello spostamento non esiste: { $path }

journal.cli.host.config.missing.parent.move.target.parent.does.not = solstone journal config: rifiutato: la directory superiore della destinazione dello spostamento non esiste: { $directory }

journal.cli.host.config.plan.action.action = azione:  { $action }

journal.cli.host.config.plan.current.left.intact.re.adopt = il diario attuale resta intatto. per riadottarlo più tardi: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.plan.current.path.state = attuale: { $path } ({ $state })

journal.cli.host.config.plan.dry.run.yes.nothing.will = prova a vuoto: sì; non verrà modificato niente

journal.cli.host.config.plan.filesystem.different.devices = filesystem: dispositivi diversi

journal.cli.host.config.plan.filesystem.same.device = filesystem: stesso dispositivo

journal.cli.host.config.plan.move = sposta

journal.cli.host.config.plan.plan.summary = { $literal_1 } - riepilogo del piano

journal.cli.host.config.plan.proceed = procedi

journal.cli.host.config.plan.re.run.with.yes.proceed = esegui di nuovo con { $literal_1 } per procedere

journal.cli.host.config.plan.switch = passa

journal.cli.host.config.plan.target.path.state = destinazione:  { $path } ({ $state })

journal.cli.host.config.preflight.coordinates.saved.location.not.recognized.or = la posizione salvata del diario non è riconosciuta o non è stato possibile leggerla; le impostazioni non sono state modificate

journal.cli.host.config.preflight.coordinates.settings.could.not.be.read = non è stato possibile leggere le tue impostazioni; le impostazioni non sono state modificate: { $error }

journal.cli.host.config.prepare.folder.relocation.requires.managed.posix = la ricollocazione della cartella del diario richiede un'installazione { $literal_1 } gestita

journal.cli.host.config.read.wrapper.targets.cannot.read.path.e = solstone journal config: rifiutato: non è stato possibile leggere { $path }: { $e }

journal.cli.host.config.read.wrapper.targets.location.changed.while.this.command = la posizione del diario è cambiata mentre questo comando era in attesa; riprova a cambiare la cartella

journal.cli.host.config.refusal.current.current.state.and.target = solstone journal config: rifiutato: l'attuale è { $current_state } e la destinazione è { $target_state }; flag validi: { $flags }

journal.cli.host.config.rewrite.preflighted.with.path.not.utf8 = il percorso del diario non è { $literal_1 }

journal.cli.host.config.run.move.could.not.stop.service.before = solstone journal config: non è stato possibile fermare il servizio prima dello spostamento ({ $error })

journal.cli.host.config.run.move.could.not.stop.service.before.stop.service.before.move = solstone journal config: non è stato possibile fermare il servizio prima dello spostamento

journal.cli.host.config.run.move.move.failed.e = solstone journal config: non è stato possibile spostare: { $e }

journal.cli.host.config.run.move.moved.wrapper.updated.service.restarted = diario spostato; involucro aggiornato; servizio riavviato.

journal.cli.host.config.run.move.remains.at.target.could.not = il diario resta in { $target }; non è stato possibile finire di salvare la nuova posizione: { $e }; riprova: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.remains.at.target.retry.path = il diario resta in { $target }; riprova: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.service.installed.but.not.running = servizio installato ma non in esecuzione; diario spostato; involucro aggiornato.

journal.cli.host.config.run.move.service.not.installed.moved.wrapper = servizio non installato; diario spostato; involucro aggiornato.

journal.cli.host.config.run.move.wrapper.updated.target.but.service = involucro aggiornato su { $target } ma non è stato possibile avviare il servizio; riavvia manualmente

journal.cli.host.config.run.switch.cannot.create.path.e = solstone journal config: rifiutato: non è stato possibile creare { $path }: { $e }

journal.cli.host.config.run.switch.cannot.inspect.managed.wrappers = solstone journal config: rifiutato: non è stato possibile ispezionare gli involucri gestiti: { $error }

journal.cli.host.config.run.switch.cannot.lock.managed.wrappers = solstone journal config: rifiutato: non è stato possibile bloccare gli involucri gestiti: { $error }

journal.cli.host.config.run.switch.could.not.finish.saving.new = non è stato possibile finire di salvare la nuova posizione del diario: { $message }; riprova: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.finish.saving.new.journal.path.switch.yes = non è stato possibile finire di salvare la nuova posizione del diario: { $e }; riprova: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.stop.service.before = non è stato possibile fermare il servizio prima di cambiare la cartella del diario

journal.cli.host.config.run.switch.now.at.target.could.not = il tuo diario ora è in { $target }; non è stato possibile aggiornare il suo servizio: { $error }; riprova: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.service.installed.but.not.running = servizio installato ma non in esecuzione; involucro aggiornato.

journal.cli.host.config.run.switch.service.not.installed.wrapper.updated = servizio non installato; involucro aggiornato.

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service = solstone journal config: involucro riscritto su { $target } ma non è stato possibile eseguire il riavvio del servizio del diario ({ $error }); riavvia manualmente

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service.investigate.and.restart.manually = solstone journal config: involucro riscritto su { $target } ma '{ $literal_1 }' è uscito con { $code }; indaga e riavvia manualmente

journal.cli.host.config.run.switch.wrapper.updated.service.restarted = involucro aggiornato; servizio riavviato.

journal.cli.host.config.run.unsupported.folder.service.action = azione di servizio non supportata per la cartella del diario

journal.cli.host.config.service.summary.service.installed.and.running.will = servizio: installato e in esecuzione; si fermerà, sposterà, riscriverà l'involucro, riavvierà

journal.cli.host.config.service.summary.service.installed.and.running.will.will.rewrite.wrapper.restart = servizio: installato e in esecuzione; riscriverà l'involucro, riavvierà

journal.cli.host.config.service.summary.service.installed.but.not.running = servizio: installato ma non in esecuzione; sposterà e riscriverà l'involucro

journal.cli.host.config.service.summary.service.installed.but.not.running.running.will.rewrite.wrapper = servizio: installato ma non in esecuzione; riscriverà l'involucro

journal.cli.host.config.service.summary.service.not.installed.will.move = servizio: non installato; sposterà e riscriverà l'involucro

journal.cli.host.config.service.summary.service.not.installed.will.rewrite = servizio: non installato; riscriverà l'involucro

journal.cli.host.config.show.next.next = prossimo: { $next }

journal.cli.host.config.show.source.built.in.default = predefinito integrato ({ $literal_1 })

journal.cli.host.config.show.source.source.tree.fallback = ripiego dell'albero dei sorgenti

journal.cli.host.config.show.source.user.toml = configurazione utente ({ $literal_1 })

journal.cli.host.config.state.active = attivo

journal.cli.host.config.state.not.active = non attivo

journal.cli.host.config.usage = uso: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }

journal.cli.host.config.wrapper.next.move.path.aside.then.run = sposta { $path } da parte, poi esegui '{ $literal_1 }' dal checkout dei sorgenti di solstone

journal.cli.host.config.wrapper.refusal.wrapper.not.managed.wrapper.run = solstone journal config: rifiutato: { $wrapper } non è un involucro gestito (esegui '{ $literal_1 }' dal checkout dei sorgenti di solstone per installare prima l'involucro)

journal.cli.host.contract.build.bundle.browser.schema.bytes.diverged.from = contratto: i byte dello schema del browser sono divergiti da { $literal_1 }

journal.cli.host.contract.build.bundle.cannot.read.browser.schema.path = contratto: non è stato possibile leggere lo schema del browser { $path }: { $error }

journal.cli.host.contract.build.bundle.invalid.json.schema.path = contratto: { $literal_1 } Schema non valido { $path }: { $error }

journal.cli.host.contract.build.bundle.missing.required.schema.sources.sources = contratto: fonti dello schema obbligatorie mancanti: { $sources }

journal.cli.host.contract.build.bundle.path.duplicate.format.id.format = contratto: { $path }: id di formato del contratto del diario duplicato '{ $format_id }'

journal.cli.host.contract.build.bundle.path.format.id.must.be = contratto: { $path }: { $literal_1 } deve essere una stringa

journal.cli.host.contract.build.bundle.schema.must.be.json.object = contratto: lo schema deve essere un oggetto { $literal_1 }: { $path }

journal.cli.host.contract.build.help = uso: { $syntax }{ $line_break }{ $line_break }Crea il pacchetto del contratto del diario.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_15 }Controlla se il pacchetto è aggiornato senza scrivere.{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Checkout del contratto o radice del pacchetto installato.{ $line_break }

journal.cli.host.contract.build.usage = uso: { $syntax }{ $line_break }

journal.cli.host.contract.check.help = uso: { $syntax }{ $line_break }{ $line_break }Controlla il pacchetto e i file del diario rispetto ai loro schemi.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }Radice del diario aggiuntiva da convalidare (ripetibile).{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Checkout del contratto o radice del pacchetto installato.{ $line_break }

journal.cli.host.contract.check.usage = uso: { $syntax }{ $line_break }

journal.cli.host.contract.classify.breaking.changes.bundle.malformed = il pacchetto del contratto del diario di solstone è malformato

journal.cli.host.contract.classify.breaking.changes.format.removed.format = { $format }: formato rimosso

journal.cli.host.contract.classify.breaking.changes.format.removed.key.field.field = { $format }: campo chiave rimosso '{ $field }'

journal.cli.host.contract.classify.breaking.changes.format.removed.producer.path.path = { $format }: percorso del produttore rimosso '{ $path }'

journal.cli.host.contract.discover.schema.sources.path.missing.meta = contratto: { $path }: manca { $contract_meta }

journal.cli.host.contract.fixture.journal.fixture.missing.path = contratto: fixture del diario mancante: { $path }

journal.cli.host.contract.format.errors.invalid.json.schema = { $literal_1 } Schema non valido: { $error }

journal.cli.host.contract.from.root.package.tree.not.found.path = contratto: albero del pacchetto solstone non trovato: { $path }

journal.cli.host.contract.help = uso: { $syntax }{ $line_break }{ $line_break }Crea e convalida il pacchetto del contratto del diario.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_15 }Crea il pacchetto del contratto.{ $line_break }{ $spaces_4 }{ $subcommand_8 }{ $spaces_15 }Controlla il pacchetto e i file del diario.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.contract.load.json.invalid.json.in.path = contratto: { $literal_1 } non valido in { $path }: { $error }

journal.cli.host.contract.metadata.path.allowed.producers.must.be = contratto: { $path }: { $literal_1 } deve essere un elenco

journal.cli.host.contract.metadata.path.key.fields.must.be = contratto: { $path }: { $literal_1 } deve essere un elenco

journal.cli.host.contract.metadata.path.missing.metadata.key = contratto: { $path }: metadati del contratto mancanti: { $key }

journal.cli.host.contract.resolve.could.not.inspect.current.executable = contratto: non è stato possibile ispezionare l'eseguibile attuale: { $error }

journal.cli.host.contract.resolve.from.executable.could.not.locate.installed.package = contratto: non è stato possibile trovare il pacchetto solstone installato o il checkout dei sorgenti da { $path }

journal.cli.host.contract.run.build.cannot.create.bundle.directory = contratto: non è stato possibile creare la directory del pacchetto: { $error }

journal.cli.host.contract.run.build.cannot.write.path = contratto: non è stato possibile scrivere { $path }: { $error }

journal.cli.host.contract.run.build.path.stale.run.build = { $path } non è aggiornato; esegui `{ $literal_1 }`

journal.cli.host.contract.run.build.wrote.path = scritto { $path }

journal.cli.host.contract.run.check.bundle.artifact.missing.path = contratto: artefatto del pacchetto mancante: { $path }

journal.cli.host.contract.run.check.no.covered.files.found.under = contratto: nessun file coperto dal contratto trovato sotto { $path }

journal.cli.host.contract.run.check.tree.not.found.path.expected = contratto: albero del diario non trovato: { $path } (atteso { $expected })

journal.cli.host.contract.usage = uso: { $syntax }{ $line_break }

journal.cli.host.contract.validate.contract.file.unsupported.file.kind.kind = tipo di file del contratto non supportato: { $kind }

journal.cli.host.contract.validate.headered.jsonl.headered.jsonl.requires.header.line = { $literal_1 } richiede una riga di intestazione

journal.cli.host.contract.validate.headered.jsonl.invalid.utf8 = { $literal_1 } non valido: { $error }

journal.cli.host.contract.validate.headered.jsonl.line.must.be.json.object = la riga deve essere un oggetto { $literal_1 }

journal.cli.host.contract.validate.headered.jsonl.missing.schema.definition = definizione dello schema mancante

journal.cli.host.contract.validate.json.invalid.json = { $literal_1 } non valido: { $error }

journal.cli.host.contract.validated = { $quantity ->
    [one] convalidato { $display_count } file coperto dal contratto sotto { $root }
    [many] convalidati { $display_count } di file coperti dal contratto sotto { $root }
   *[other] convalidati { $display_count } file coperti dal contratto sotto { $root }
}

journal.cli.host.contract.walk.four.levels.cannot.read.entry = contratto: non è stato possibile leggere l'elemento del diario: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.path = contratto: non è stato possibile leggere { $path }: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.schema.entry = contratto: non è stato possibile leggere l'elemento dello schema: { $error }

journal.cli.host.convey.help = uso: { $syntax }{ $line_break }{ $line_break }{ $term } interfaccia web{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Porta su cui servire{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_2 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_4 }Attiva il registro di debug{ $line_break }

journal.cli.host.convey.usage = uso: { $syntax }{ $line_break }

journal.cli.host.cortex.help = uso: { $syntax }{ $line_break }{ $line_break }{ $term } { $term_4 } Gestore dei talenti{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_2 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_4 }Attiva il registro di debug{ $line_break }

journal.cli.host.cortex.service.cortex.service.failed = servizio cortex non riuscito: { $error }

journal.cli.host.cortex.usage = uso: { $syntax }{ $line_break }

journal.cli.host.down.help = uso: { $syntax }{ $line_break }{ $line_break }Ferma il servizio del diario.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.facet.candidates.help = uso: { $syntax }{ $line_break }{ $line_break }Salva le facette candidate ricorrenti per la revisione.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Attiva il registro di debug{ $line_break }

journal.cli.host.facet.candidates.recorded = { $quantity ->
    [one] Salvata/aggiornata { $display_count } facetta candidata.
    [many] Salvate/aggiornate { $display_count } di facette candidate.
   *[other] Salvate/aggiornate { $display_count } facette candidate.
}

journal.cli.host.facet.candidates.run.facet.candidates = solstone journal facet-candidates: errore: { $error }

journal.cli.host.facet.candidates.run.facet.candidates.root.path.not = solstone journal facet-candidates: errore: la radice del diario { $path } non è una directory

journal.cli.host.facet.candidates.usage = uso: { $syntax }{ $line_break }

journal.cli.host.grab.help = uso: { $syntax }{ $line_break }{ $line_break }Percorri i fotogrammi dello schermo presi e, in opzione, scrivi le immagini dei fotogrammi.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_11 }Token di percorso: { $path_tokens }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_6 }Scrivi qui l'immagine del fotogramma selezionato ({ $extension }, { $extension_12 }, { $extension_13 } o{ $line_break }{ $spaces_17 }{ $extension_15 }).{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_8 }Sostituisci un percorso di output esistente.{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_9 }Emetti { $term } invece di una tabella o dell'output semplice.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_2 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_4 }Attiva il registro di debug{ $line_break }

journal.cli.host.grab.usage = uso: { $syntax }{ $line_break }

journal.cli.host.health.callosum.clients = { $quantity ->
    [one] Callosum: { $clients } client
    [many] Callosum: { $clients } di client
   *[other] Callosum: { $clients } client
}

journal.cli.host.health.ced.ready = l'engine di tag audio ced.cpp e il modello sono pronti

journal.cli.host.health.ced.unsupported = installazione ced: piattaforma { $os }/{ $arch } non supportata; si saltano le risorse di tag audio ced

journal.cli.host.health.crash.attempts = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } tentativo di riavvio
    [many] { $space_2 }{ $name } { $attempts } di tentativi di riavvio
   *[other] { $space_2 }{ $name } { $attempts } tentativi di riavvio
}

journal.cli.host.health.crash.reason = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } tentativo di riavvio{ $space_2 }{ $reason }
    [many] { $space_2 }{ $name } { $attempts } di tentativi di riavvio{ $space_2 }{ $reason }
   *[other] { $space_2 }{ $name } { $attempts } tentativi di riavvio{ $space_2 }{ $reason }
}

journal.cli.host.health.crashed = { $line_break }Arresto anomalo:{ $line_break }

journal.cli.host.health.heartbeat.ok = Battito: ok{ $line_break }

journal.cli.host.health.heartbeat.stale = { $line_break }Battito: NON AGGIORNATO ({ $stale })

journal.cli.host.health.held = { $space_2 }trattenuto { $partition } { $reason }

journal.cli.host.health.help = uso: { $syntax }{ $line_break }{ $line_break }Mostra lo stato del servizio conservato dal supervisore.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }attiva l'output di debug{ $line_break }

journal.cli.host.health.logs.help = uso: { $syntax }{ $line_break }{ $line_break }Vedi i registri operativi del servizio.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_18 }numero di righe da mostrare (predefinito: 5){ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_20 }segui l'output del registro{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }mostra le righe a partire da { $argument }{ $line_break }{ $spaces_2 }{ $option_13 }{ $spaces_8 }mostra le righe di un solo servizio{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }mostra le righe corrispondenti a { $argument_16 }{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }attiva l'output di debug{ $line_break }

journal.cli.host.health.logs.usage = uso: { $syntax }{ $line_break }

journal.cli.host.health.media.absent = Processore dei contenuti multimediali: non segnala

journal.cli.host.health.media.depth.stale = Processore dei contenuti multimediali: { $depth } in attesa (non aggiornato, { $seconds } s dal faro)

journal.cli.host.health.media.idle = Processore dei contenuti multimediali: inattivo

journal.cli.host.health.media.pending = Processore dei contenuti multimediali: { $depth } in attesa

journal.cli.host.health.media.stale = Processore dei contenuti multimediali: non aggiornato ({ $seconds } s dal faro)

journal.cli.host.health.not.running = Non è stato possibile connettersi: il tuo diario non è in esecuzione{ $line_break }

journal.cli.host.health.queue = { $space_2 }in coda { $name } { $count }

journal.cli.host.health.queue.hold = { $space_2 }blocco-coda { $reason } { $details }

journal.cli.host.health.rfdetr.assets = Il rilevamento degli oggetti è degradato perché le sue risorse RF-DETR non sono disponibili. Le descrizioni dello schermo continueranno. Usa `{ $command }` per controllare o riparare le risorse RF-DETR.

journal.cli.host.health.rfdetr.package = il rilevamento degli oggetti non è disponibile. ripara o reinstalla il tuo diario.

journal.cli.host.health.rfdetr.ready = l'engine di rilevamento degli oggetti rf-detr.cpp e il modello sono pronti

journal.cli.host.health.rfdetr.unsupported = installazione rf-detr: piattaforma { $os }/{ $arch } non supportata; si saltano le risorse di rilevamento degli oggetti rf-detr

journal.cli.host.health.runtime = Non è stato possibile connettersi: ambiente di esecuzione dello stato non disponibile: { $message }{ $line_break }

journal.cli.host.health.service.days = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $days } d

journal.cli.host.health.service.days.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $days } d { $hours } h

journal.cli.host.health.service.days.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $days } d { $hours } h { $minutes } min

journal.cli.host.health.service.days.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $days } d { $minutes } min

journal.cli.host.health.service.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $hours } h

journal.cli.host.health.service.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $hours } h { $minutes } min

journal.cli.host.health.service.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $minutes } min

journal.cli.host.health.service.seconds = { $space_2 }{ $name } pid { $pid }{ $space_2 }tempo di attività { $seconds } s

journal.cli.host.health.services = Servizi:{ $line_break }

journal.cli.host.health.socket.encoding = Non è stato possibile connettersi: il percorso del socket di callosum non è UTF-8 valido{ $line_break }

journal.cli.host.health.socket.inspect = Non è stato possibile connettersi: il socket di callosum non è ispezionabile: { $reason }{ $line_break }

journal.cli.host.health.socket.missing = Non è stato possibile connettersi: socket di callosum non trovato in { $path }{ $line_break }

journal.cli.host.health.socket.type = il percorso non è un socket Unix

journal.cli.host.health.status.error = Non è stato possibile connettersi: errore di stato del supervisore: { $service }: { $reason }{ $line_break }

journal.cli.host.health.status.invalid = Non è stato possibile connettersi: il supervisore ha inviato un payload di stato non valido in { $path }

journal.cli.host.health.task.running = { $space_2 }{ $name } { $duration } s

journal.cli.host.health.task.slow = { $space_2 }{ $name } { $duration } s{ $space_2 }LENTO (cap { $limit } s)

journal.cli.host.health.task.stuck = { $space_2 }{ $name } { $duration } s{ $space_2 }IN STALLO (cap { $limit } s)

journal.cli.host.health.tasks = Compiti:{ $line_break }

journal.cli.host.health.tasks.empty = Compiti: nessuno{ $line_break }

journal.cli.host.health.timeout = tempo scaduto in attesa dello stato del supervisore (10 s){ $line_break }

journal.cli.host.health.timeout.malformed.frames = { $quantity ->
    [one] tempo scaduto in attesa dello stato del supervisore (10 s; scartato { $count } fotogramma malformato){ $line_break }
    [many] tempo scaduto in attesa dello stato del supervisore (10 s; scartati { $count } di fotogrammi malformati){ $line_break }
   *[other] tempo scaduto in attesa dello stato del supervisore (10 s; scartati { $count } fotogrammi malformati){ $line_break }
}

journal.cli.host.health.usage = uso: { $syntax }{ $line_break }

journal.cli.host.heartbeat.error = { $command }: { $detail }

journal.cli.host.heartbeat.help = uso: { $syntax }{ $line_break }{ $line_break }Esegui il passaggio deterministico di riparazione dello stato{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_5 }Esegui il controllo completo indipendentemente dalla recenza{ $line_break }

journal.cli.host.heartbeat.oplog = non è stato possibile creare l'oplog heartbeat: { $detail }

journal.cli.host.heartbeat.usage = uso: { $syntax }{ $line_break }

journal.cli.host.hosted.service.hosted.service.admission.refused = ammissione del servizio ospitato { $service } rifiutata: { $error }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.day = non è stato possibile mostrare l'avviso del giorno di importazione: { $error }{ $line_break }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.size = non è stato possibile mostrare la dimensione dell'importazione: { $error }{ $line_break }

journal.cli.host.install.models.coreml.disclosure.parakeet.assets.downloading.parakeet.tdt = risorse di parakeet: scaricamento del modello { $model } { $literal_4 } ({ $literal_3 }) da { $literal_2 }. vedi { $literal_1 }.

journal.cli.host.install.models.help = uso: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }{ $line_break }installa e verifica i modelli { $argument } inclusi di { $term } ({ $argument_8 } locale più le risorse incluse{ $line_break }{ $term_9 }{ $path }). l'azione predefinita controlla gli artefatti { $argument_8 } locali e{ $line_break }scarica se mancano; { $flag } riscarica; { $flag_12 } verifica soltanto ed esce{ $line_break }con un valore diverso da zero se c'è un problema.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_15 }verifica le risorse incluse e gli artefatti { $argument_8 } locali senza{ $line_break }{ $spaces_24 }scaricare.{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_15 }ignora la prontezza e riscarica{ $path_20 } gli artefatti { $argument_8 }{ $line_break }{ $spaces_24 }locali.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_7 }installa o verifica solo i modelli inclusi richiesti per{ $line_break }{ $spaces_24 }la prontezza del diario; non scaricare i fornitori facoltativi.{ $line_break }{ $spaces_2 }{ $flag_23 } { $choices }{ $line_break }{ $spaces_24 }variante del diario da installare o verificare. { $literal } rispetta{ $line_break }{ $spaces_24 }{ $argument_26 } su { $literal_27 }{ $path_28 }, poi { $literal } rileva automaticamente.{ $line_break }

journal.cli.host.install.models.install.required.rfdetr.rf.detr.install.unsupported.platform = { $literal_1 } installazione: piattaforma { $os }/{ $arch } non supportata; salto delle risorse di rilevamento oggetti { $literal_1 }

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.percent = modello parakeet { $model_name }: { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.verifying = modello parakeet { $model_name }: verifica dello scaricamento in corso

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.download.starting = configurazione di parakeet: avvio dello scaricamento

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.downloading.required.files = configurazione di parakeet: scaricamento in corso dei file richiesti { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.preparing.parakeet.speech.model.model = preparazione in corso del modello del parlato parakeet: { $model_name }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.model = { $literal_1 } controllo non riuscito: modello mancante in { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name = { $literal_1 } controllo non riuscito: { $name } mancante in { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name.not.executable.at.path = { $literal_1 } controllo non riuscito: { $name } non eseguibile in { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.model.path.missing = percorso del modello parakeet mancante

journal.cli.host.install.models.parakeet.ready.parakeet.path.missing = percorso di parakeet mancante

journal.cli.host.install.models.parakeet.without.download.parakeet.install.unsupported.platform.os = installazione parakeet: piattaforma { $os }/{ $arch } non supportata; supportate: { $literal_1 }, { $literal_2 }

journal.cli.host.install.models.parakeet.without.download.parakeet.speech.recognition.ships.inside = parakeet: il riconoscimento vocale è incluso nell'app del diario su windows, quindi non c'è niente da scaricare

journal.cli.host.install.models.ready.line.model.ready.path = modello pronto: { $path }

journal.cli.host.install.models.resolve.variant.invalid.variant.cuda.use.cpu = { $literal_1 }={ $literal_2 } non valido; usa { $literal_3 } su { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.invalid.variant.value.use.cpu = { $literal_1 }='{ $value }' non valido; usa { $literal_3 } o { $literal_2 }

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on = variante { $literal_1 } non supportata su { $os }

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on.supported.on.os.arch = variante { $literal_1 } non supportata su { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on = variante { $literal_1 } non supportata su { $os }

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on.supported.on.os.arch = variante { $literal_1 } non supportata su { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on = variante { $literal_1 } non supportata su { $os }

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on.supported.on.os.arch = variante { $literal_1 } non supportata su { $os }/{ $arch }

journal.cli.host.install.models.rfdetr.bundled.asset.disclosure.rf.detr.assets.verifying.bundled = risorse di rf-detr: verifica del modello incluso { $literal_2 } { $version } ({ $literal_3 }) e dei pesi { $literal_4 } nano { $literal_5 } ({ $literal_3 }). vedi { $literal_1 }.

journal.cli.host.install.models.rfdetr.repair.object.detection.unavailable.repair.or = il rilevamento di oggetti non è disponibile. ripara o reinstalla il tuo diario. { $error }

journal.cli.host.install.models.run.inner.speech.install.already.running.wait = l'installazione del parlato è già in esecuzione; attendi che finisca

journal.cli.host.install.models.run.inner.with.bundled.asset.verification.failed = verifica della risorsa inclusa non riuscita: { $error }

journal.cli.host.install.models.run.inner.with.observing.parakeet.install.state.suffix = osservazione dell'installazione parakeet: { $state }{ $suffix }

journal.cli.host.install.models.run.inner.with.parakeet.install.already.running.for = installazione parakeet già in esecuzione per una destinazione diversa

journal.cli.host.install.models.run.inner.with.parakeet.install.failed = installazione parakeet non riuscita

journal.cli.host.install.models.run.inner.with.timed.out.observing.parakeet.install = tempo scaduto nell'osservazione dell'installazione parakeet

journal.cli.host.install.models.usage = uso: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }

journal.cli.host.install.provider.dispatch.message.provider.install.failed = installazione { $provider } non riuscita

journal.cli.host.install.provider.global.local.model.assets.downloading.llama = risorse del modello locale: scaricamento dell'ambiente di esecuzione { $literal_4 } ({ $literal_7 }; il build { $literal_6 } contiene anche componenti dell'ambiente di esecuzione { $literal_5 }-licenziati) e del modello ({ $literal_3 }) da { $literal_2 }. vedi { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.and = il ragionamento locale scarica il modello e il proiettore visivo ({ $literal_3 }) da { $literal_2 } quando necessario. l'ambiente di esecuzione è incluso con il diario. vedi { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.apache = il ragionamento locale scarica il modello ({ $literal_3 }) da { $literal_2 }. l'ambiente di esecuzione { $literal_4 } è incluso con il diario. vedi { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.and.its.speech = { $literal_2 } e il suo modello del parlato sono inclusi con il diario su windows, quindi non c'è niente da installare. vedi { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.can.t.run = { $literal_1 } non può essere eseguito su un mac. un mac trascrive invece con il suo modello { $literal_2 }.

journal.cli.host.install.provider.global.parakeet.cpp.fetches.two.artifacts = { $literal_4 } scarica due artefatti nella cache del fornitore di questo diario prima di poter essere eseguito, entrambi da { $literal_2 }: il binario server { $literal_3 } ({ $literal_6 }) e il modello del parlato ({ $literal_5 }). vedi { $literal_1 }.

journal.cli.host.install.provider.help = uso: { $syntax }{ $line_break }{ $line_break }Installa o riprova un ambiente di esecuzione del fornitore.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Fornitore da installare: '{ $literal }' o '{ $literal_7 }'.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.install.provider.local.target.sha.local.fingerprint.dispatch.omitted.target = lo smistamento dell'impronta locale ha omesso target_fingerprint_sha256

journal.cli.host.install.provider.observe.existing.with.provider.install.already.running.for = installazione { $provider } già in esecuzione per una destinazione diversa

journal.cli.host.install.provider.observe.existing.with.timed.out.observing.provider.install = tempo scaduto nell'osservazione dell'installazione { $provider }

journal.cli.host.install.provider.progress.line.observing.provider.install.state.suffix = osservazione dell'installazione { $provider }: { $state }{ $suffix }

journal.cli.host.install.provider.run.inner.with.parakeet.already.installed = parakeet già installato

journal.cli.host.install.provider.run.inner.with.unsupported.provider.provider.supported.local = fornitore non supportato { $provider }; supportati: { $literal_1 }

journal.cli.host.install.provider.run.local.inner.with.platform.and.target.sha.local.already.installed = local già installato

journal.cli.host.install.provider.unavailable.status.error.could.not.read.persisted.provider = non è stato possibile leggere lo stato persistente dell'installazione { $provider }: { $error }

journal.cli.host.install.provider.usage = uso: { $syntax }{ $line_break }

journal.cli.host.installation.executable = non è stato possibile risolvere l'eseguibile corrente: { $error }

journal.cli.host.installation.executable.parent = l'eseguibile corrente non ha una directory superiore

journal.cli.host.installation.recovery = non è stato possibile verificare questa installazione.{ $line_break }esegui `{ $setup }` per controllarla. se la configurazione iniziale finisce correttamente, riprova.{ $line_break }dettagli: { $detail }

journal.cli.host.installation.root = non è stato possibile risolvere la radice dell'identità di installazione da { $directory }

journal.cli.host.logs.count = conteggio non valido: { $detail }

journal.cli.host.logs.follow.descriptor = catalogo non riuscito per { $path }: il descrittore oplog è cambiato durante il seguito

journal.cli.host.logs.follow.empty = Nessun file di registro trovato.

journal.cli.host.logs.follow.failed = fallimento di { $operation } per { $path }

journal.cli.host.logs.follow.root = catalogo non riuscito per { $path }: radice del diario non disponibile

journal.cli.host.logs.follow.source = fallimento di { $operation } per { $path }: { $source }

journal.cli.host.logs.grep.compile = non è stato possibile compilare il pattern regex

journal.cli.host.logs.grep.invalid = pattern regex non valido all'offset di byte { $offset }

journal.cli.host.logs.grep.unsupported = funzionalità regex non supportata '{ $family }' all'offset di byte { $offset }

journal.cli.host.logs.health.error = registri di stato: { $detail }

journal.cli.host.logs.health.usage = { $command }: errore: { $detail }

journal.cli.host.logs.service.empty = === registri del servizio === (nessuna foglia oplog)

journal.cli.host.logs.service.error = registri del servizio: { $detail }

journal.cli.host.logs.service.heading = === registri del servizio ===

journal.cli.host.logs.service.operation = fallimento di { $operation }: { $source }

journal.cli.host.logs.service.unsupported = Errore: piattaforma non supportata '{ $platform }'

journal.cli.host.logs.since.invalid = Tempo non valido: '{ $input }'. Usa ad es. { $examples }

journal.cli.host.logs.since.overflow = int Python troppo grande per la conversione in int C

journal.cli.host.main.windows.service.admission.failed = ammissione del servizio windows non riuscita: { $error }

journal.cli.host.malformed.jsonl.warning.solstone.observe.utils.invalid.json.at.line.in = { $literal_warning_solstone_observe_utils }:{ $literal_json } non valido alla riga { $line } in { $path }: { $error }

journal.cli.host.mcp.activity.activity = { $command_solstone_journal_mcp_activity }: { $error }

journal.cli.host.mcp.activity.activity.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_activity } non è compilato in questo build

journal.cli.host.mcp.activity.activity.unknown.outcome = { $command_solstone_journal_mcp_activity }: esito sconosciuto { $value }

journal.cli.host.mcp.activity.activity.unknown.tool = { $command_solstone_journal_mcp_activity }: strumento sconosciuto { $value }

journal.cli.host.mcp.activity.bytes = { $space_4 }byte:{ $space_3 }{ $byte_count }

journal.cli.host.mcp.activity.created = { $space_4 }creato: { $created }

journal.cli.host.mcp.activity.help = uso: { $syntax }{ $line_break }{ $spaces_37 }{ $syntax_4 }{ $line_break }{ $spaces_37 }{ $syntax_5 }{ $line_break }{ $line_break }Mostra che cosa hanno chiesto le connessioni e come è finita ogni richiesta.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }solo una connessione con token bearer, per la sua etichetta{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }solo una connessione { $term }, per il suo id client{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }con { $flag }, quale concessione, quando un id client ne ha diverse{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_10 }solo le richieste a questo strumento{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_4 }solo le richieste finite in questo modo{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_2 }da questo giorno{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_4 }fino a questo giorno incluso{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_12 }al massimo N richieste, da 1 a 1000 (predefinito: 20){ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_15 }stampa { $term_25 }{ $line_break }

journal.cli.host.mcp.activity.no.match = { $examined_quantity ->
    [one] Nessuna corrispondenza nel { $examined } registro esaminato; più indietro può essercene ancora.
    [many] Nessuna corrispondenza nei { $examined } di registri esaminati; più indietro può essercene ancora.
   *[other] Nessuna corrispondenza nei { $examined } registri esaminati; più indietro può essercene ancora.
}

journal.cli.host.mcp.activity.no.recorded.mcp.activity.matches = Nessuna attività { $literal_mcp } registrata corrisponde.

journal.cli.host.mcp.activity.origin = { $space_4 }origine:{ $space_2 }{ $origin }

journal.cli.host.mcp.activity.raise.limit.or.narrow.day.from.day.to.to.see = Alza { $literal_limit } o restringi { $literal_day_from }/{ $literal_day_to } per vederne altri.

journal.cli.host.mcp.activity.reason = { $space_4 }motivo:{ $space_2 }{ $reason }

journal.cli.host.mcp.activity.record = { $space_4 }registro:{ $space_2 }{ $day }/{ $segment }

journal.cli.host.mcp.activity.request = { $space_4 }richiesta: { $rendered }

journal.cli.host.mcp.activity.request.arguments.exceeded.the.record.bound.and.were.not.stored = { $space_4 }richiesta: (gli argomenti hanno superato il limite del registro e non sono stati salvati)

journal.cli.host.mcp.activity.request.legacy = { $space_4 }richiesta: (schema 1: non registrata)

journal.cli.host.mcp.activity.result = { $count_quantity ->
    [one] { $space_4 }risultato:{ $space_2 }{ $count } risultato, hash { $digest }
    [many] { $space_4 }risultato:{ $space_2 }{ $count } di risultati, hash { $digest }
   *[other] { $space_4 }risultato:{ $space_2 }{ $count } risultati, hash { $digest }
}

journal.cli.host.mcp.activity.result.targets = { $count_quantity ->
    [one] { $space_4 }risultato:{ $space_2 }{ $count } risultato, hash { $digest }, obiettivi { $targets }
    [many] { $space_4 }risultato:{ $space_2 }{ $count } di risultati, hash { $digest }, obiettivi { $targets }
   *[other] { $space_4 }risultato:{ $space_2 }{ $count } risultati, hash { $digest }, obiettivi { $targets }
}

journal.cli.host.mcp.activity.result.truncated = { $count_quantity ->
    [one] { $space_4 }risultato:{ $space_2 }{ $count } risultato, hash { $digest }, obiettivi { $targets }, ...
    [many] { $space_4 }risultato:{ $space_2 }{ $count } di risultati, hash { $digest }, obiettivi { $targets }, ...
   *[other] { $space_4 }risultato:{ $space_2 }{ $count } risultati, hash { $digest }, obiettivi { $targets }, ...
}

journal.cli.host.mcp.activity.schema.connection = (schema 1: nessuna connessione registrata)

journal.cli.host.mcp.activity.shown.complete = { $shown_quantity ->
    [one] { $shown } registro mostrato di { $examined } all'esame. { $counts }
    [many] { $shown } di registri mostrati di { $examined } all'esame. { $counts }
   *[other] { $shown } registri mostrati di { $examined } all'esame. { $counts }
}

journal.cli.host.mcp.activity.shown.partial = { $shown_quantity ->
    [one] { $shown } registro mostrato di { $examined } all'esame (più indietro può essercene ancora). { $counts }
    [many] { $shown } di registri mostrati di { $examined } all'esame (più indietro può essercene ancora). { $counts }
   *[other] { $shown } registri mostrati di { $examined } all'esame (più indietro può essercene ancora). { $counts }
}

journal.cli.host.mcp.activity.unreadable = { $count_quantity ->
    [one] non è stato possibile analizzare { $count } registro nell'intervallo e non è contato sopra.
    [many] non è stato possibile analizzare { $count } di registri nell'intervallo e non sono contati sopra.
   *[other] non è stato possibile analizzare { $count } registri nell'intervallo e non sono contati sopra.
}

journal.cli.host.mcp.help = uso: { $syntax }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $line_break }sottocomandi:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }Esegui l'endpoint { $term }{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_12 }Esegui la connessione per gli agenti su questo computer{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_16 }Mostra lo stato dell'endpoint { $term }{ $line_break }{ $spaces_2 }{ $subcommand_12 }{ $spaces_17 }Gestisci i token bearer { $term }{ $line_break }{ $spaces_2 }{ $subcommand_14 }{ $spaces_15 }Gestisci il codice di accoppiamento dell'agente{ $line_break }{ $spaces_2 }{ $subcommand_15 }{ $spaces_17 }Gestisci i client { $term_16 } registrati{ $line_break }{ $spaces_2 }{ $subcommand_17 }{ $spaces_12 }Gestisci i permessi di lettura delle connessioni{ $line_break }{ $spaces_2 }{ $subcommand_18 }{ $spaces_14 }Mostra che cosa hanno chiesto le connessioni e come è finita ogni richiesta{ $line_break }{ $spaces_2 }{ $subcommand_20 }{ $spaces_17 }Invoca una lettura { $term } senza aprire una porta{ $line_break }

journal.cli.host.mcp.local.door.help = uso: { $syntax }{ $line_break }{ $line_break }Esegui la connessione per gli agenti su questo computer.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.mcp.local.door.local.door.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_local_door } non è compilato in questo build

journal.cli.host.mcp.local.door.mcp.local.door.failed = mcp local-door non riuscito: { $class }

journal.cli.host.mcp.oauth.error.detail = { $command_solstone_journal_mcp } { $command } { $operation }: { $error }

journal.cli.host.mcp.oauth.help = uso: { $syntax }{ $line_break }{ $line_break }Gestisci i client { $term } registrati.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $line_break }sottocomandi:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Elenca i client { $term } registrati{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_6 }Revoca un client { $term } registrato{ $line_break }

journal.cli.host.mcp.oauth.list.help = uso: { $syntax }{ $line_break }{ $line_break }Elenca i client { $term } registrati.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.mcp.oauth.oauth.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_oauth } non è compilato in questo build

journal.cli.host.mcp.oauth.oauth.revoke.no.such.oauth.client = { $command_solstone_journal_mcp_oauth_revoke }: nessun client { $literal_oauth }

journal.cli.host.mcp.oauth.revoke.help = uso: { $syntax }{ $line_break }{ $line_break }Revoca un client { $term } registrato.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_13 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_2 }il client da revocare{ $line_break }

journal.cli.host.mcp.oauth.revoked.oauth.client = Client { $literal_oauth } { $client_id } revocato.

journal.cli.host.mcp.pairing.at.expires.at = Scade alle { $expires_at }.

journal.cli.host.mcp.pairing.at.it.works.only.at.your.own.domain = Funziona solo sul tuo dominio.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer = Funziona solo per un agente su questo computer.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer.to = Funziona solo per un agente su questo computer. Per connettere un agente in un altro modo, usa { $literal_door } lan, { $literal_solstone_me } o hostname.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.your.network = Funziona solo per un agente sulla tua rete.

journal.cli.host.mcp.pairing.at.it.works.only.through.solstone.me = Funziona solo tramite { $literal_solstone_me }.

journal.cli.host.mcp.pairing.at.pairing.generate.no.hostname.is.set.up.for.this.journal = { $command_solstone_journal_mcp_pairing_generate }: nessun hostname è configurato per questo diario. configurane uno prima nell'app degli agenti.

journal.cli.host.mcp.pairing.at.pairing.generate.this.journal.s.config.could.not.be.read = { $command_solstone_journal_mcp_pairing_generate }: non è stato possibile leggere la configurazione di questo diario.

journal.cli.host.mcp.pairing.at.pairing.revoke.no.active.pairing.code.to.revoke = { $command_solstone_journal_mcp_pairing_revoke }: nessun codice di accoppiamento attivo da revocare

journal.cli.host.mcp.pairing.at.revoked.the.active.pairing.code = Codice di accoppiamento attivo revocato.

journal.cli.host.mcp.pairing.at.save.this.now.it.will.not.be.shown.again.and = Salva questo ora. Non sarà mostrato di nuovo e non può essere recuperato:

journal.cli.host.mcp.pairing.generate.help = uso: { $syntax }{ $line_break }{ $line_break }Genera un codice di accoppiamento per un agente, valido per 10 minuti e un solo uso. È mostrato una sola volta.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $flag } { $choices }{ $line_break }{ $spaces_24 }dove funziona il codice (predefinito: { $literal }, un agente su questo computer){ $line_break }

journal.cli.host.mcp.pairing.generated = Codice di accoppiamento generato, valido per 10 minuti e un solo uso.

journal.cli.host.mcp.pairing.help = uso: { $syntax }{ $line_break }{ $line_break }Gestisci il codice di accoppiamento dell'agente.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $line_break }sottocomandi:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_4 }Genera un codice di accoppiamento{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_6 }Revoca il codice di accoppiamento corrente{ $line_break }

journal.cli.host.mcp.pairing.pairing.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_pairing } non è compilato in questo build

journal.cli.host.mcp.pairing.revoke.help = uso: { $syntax }{ $line_break }{ $line_break }Revoca il codice di accoppiamento corrente.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.mcp.permission.categories = Categorie: { $categories }

journal.cli.host.mcp.permission.clear.help = uso: { $syntax }{ $line_break }{ $line_break }Cancella il permesso di lettura di una connessione.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }una connessione con token bearer, per la sua etichetta{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }una connessione { $term }, per il suo id client{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }con { $flag }, quale concessione, quando un id client ne ha diverse{ $line_break }

journal.cli.host.mcp.permission.cleared.permission.for.connection = Permesso cancellato per la connessione { $connection }.

journal.cli.host.mcp.permission.connection = Connessione: { $connection }

journal.cli.host.mcp.permission.evaluation = Valutazione: { $evaluation }

journal.cli.host.mcp.permission.generation = Generazione: { $generation }

journal.cli.host.mcp.permission.help = uso: { $syntax }{ $line_break }{ $line_break }Gestisci i permessi di lettura delle connessioni.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $line_break }sottocomandi:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Mostra i permessi di lettura, per ogni connessione o una{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_9 }Imposta che cosa può leggere una connessione{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_7 }Cancella il permesso di lettura di una connessione{ $line_break }

journal.cli.host.mcp.permission.no.mcp.connection.permissions.configured = Nessun permesso di connessione { $literal_mcp } configurato.

journal.cli.host.mcp.permission.no.permission.was.configured.for.connection = Nessun permesso era configurato per la connessione { $connection }.

journal.cli.host.mcp.permission.permission.clear = { $command_solstone_journal_mcp_permission_clear }: { $error }

journal.cli.host.mcp.permission.permission.management.is.not.compiled.into.this.build = la gestione di { $command_solstone_journal_mcp_permission } non è compilata in questo build

journal.cli.host.mcp.permission.permission.no.permission.configured = Permesso: nessun permesso configurato

journal.cli.host.mcp.permission.permission.set = { $command_solstone_journal_mcp_permission_set }: { $error }

journal.cli.host.mcp.permission.permission.set.choose.a.category.from = { $command_solstone_journal_mcp_permission_set }: scegli una categoria tra { $categories }

journal.cli.host.mcp.permission.permission.set.choose.places.precision = { $command_solstone_journal_mcp_permission_set }: scegli un livello di dettaglio dei luoghi tra { $levels }

journal.cli.host.mcp.permission.permission.set.places.precision.requires.places = { $command_solstone_journal_mcp_permission_set }: { $flag } richiede { $category_flag }

journal.cli.host.mcp.permission.permission.show = { $command_solstone_journal_mcp_permission_show }: { $error }

journal.cli.host.mcp.permission.places.precision = Dettaglio dei luoghi: { $precision }

journal.cli.host.mcp.permission.set.help.places.precision = { $spaces_2 }{ $option }{ $spaces_2 }dettaglio della posizione quando { $flag } è impostato: { $values } (predefinito: { $default }){ $line_break }

journal.cli.host.mcp.permission.set.help.v2 = uso: { $syntax }{ $line_break }{ $spaces_43 }{ $syntax_4 }{ $line_break }{ $spaces_43 }{ $syntax_5 }{ $line_break }{ $line_break }Imposta che cosa può leggere una connessione.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }una connessione con token bearer, per la sua etichetta{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }una connessione { $term }, per il suo id client{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }con { $flag }, quale concessione, quando un id client ne ha diverse{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_2 }un tipo che questa connessione può leggere (ripeti per ogni tipo){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_8 }solo questa facetta, per nome; ripeti per altre (predefinito: il diario intero){ $line_break }

journal.cli.host.mcp.permission.set.mcp.read.permission.for.connection.generation = Permesso di lettura { $literal_mcp } impostato per la connessione { $connection } (generazione { $generation }).

journal.cli.host.mcp.permission.show.help = uso: { $syntax }{ $line_break }{ $line_break }Mostra i permessi di lettura, per ogni connessione o per una.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }una connessione con token bearer, per la sua etichetta{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }una connessione { $term }, per il suo id client{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }con { $flag }, quale concessione, quando un id client ne ha diverse{ $line_break }

journal.cli.host.mcp.probe.help = uso: { $syntax }{ $line_break }{ $spaces_34 }{ $flag } { $argument } { $flag_6 } { $term }{ $line_break }{ $line_break }Invoca una lettura { $term_8 } come connessione, senza aprire una porta.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_8 }una connessione con token bearer, per la sua etichetta{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_4 }una connessione { $term_16 }, per il suo id client{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_2 }con { $flag_18 }, quale concessione, quando un id client ne ha diverse{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_10 }lo strumento da chiamare{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_5 }gli argomenti dello strumento, come oggetto { $term }{ $line_break }

journal.cli.host.mcp.service.help = uso: { $syntax }{ $line_break }{ $line_break }Esegui l'endpoint { $term }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.mcp.service.mcp.service.failed = servizio mcp non riuscito: { $class }

journal.cli.host.mcp.service.service.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_service } non è compilato in questo build

journal.cli.host.mcp.status.capability.compiled.in.true = capacità compilata all'interno: true

journal.cli.host.mcp.status.capability.disabled = capacità: disattivata

journal.cli.host.mcp.status.capability.enabled = capacità: attivata

journal.cli.host.mcp.status.capability.error = capacità: errore

journal.cli.host.mcp.status.help = uso: { $syntax }{ $line_break }{ $line_break }Mostra lo stato dell'endpoint { $term }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.mcp.status.lan.disabled = porta lan: disattiva alla porta { $port }

journal.cli.host.mcp.status.lan.enabled = porta lan: attivata alla porta { $port }

journal.cli.host.mcp.status.local.disabled = porta locale: non attivata a { $resource }

journal.cli.host.mcp.status.local.enabled = porta locale: attivata a { $resource }

journal.cli.host.mcp.status.mcp.capability.config.status.only.this.does.not.report.service = solo stato di capacità/configurazione { $literal_mcp }; questo non riporta la vitalità del servizio.

journal.cli.host.mcp.status.status.could.not.read.bearer.token.ledger = { $command_solstone_journal_mcp_status }: non è stato possibile leggere il registro dei token bearer: { $error }

journal.cli.host.mcp.status.status.could.not.read.mcp.capability.configuration = { $command_solstone_journal_mcp_status }: non è stato possibile leggere la configurazione di capacità { $literal_mcp }: { $error }

journal.cli.host.mcp.status.status.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_status } non è compilato in questo build

journal.cli.host.mcp.status.status.mcp.capability.configuration.is.invalid = { $command_solstone_journal_mcp_status }: la configurazione di capacità { $literal_mcp } non è valida: { $error }

journal.cli.host.mcp.status.token.count = conteggio dei token: { $count }

journal.cli.host.mcp.status.token.unavailable = conteggio dei token: non disponibile

journal.cli.host.mcp.token.create.help = uso: { $syntax }{ $line_break }{ $line_break }Crea un token bearer per una connessione. Il token è mostrato una sola volta.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }un nome per la connessione a cui il token è destinato{ $line_break }

journal.cli.host.mcp.token.created.bearer.token.for.label = Token bearer creato per l'etichetta { $label }.

journal.cli.host.mcp.token.grant.before.the.agent.connects.an.agent.that.connected.first = Concedi prima che l'agente si connetta. Un agente che si è connesso prima conserva il suo vecchio elenco di strumenti finché non si riconnette.

journal.cli.host.mcp.token.help = uso: { $syntax }{ $line_break }{ $line_break }Gestisci i token bearer { $term }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $line_break }sottocomandi:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_6 }Crea un token bearer per una connessione{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_8 }Elenca i token{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_6 }Revoca un token{ $line_break }

journal.cli.host.mcp.token.list.help = uso: { $syntax }{ $line_break }{ $line_break }Elenca l'etichetta di ogni token e quando è stato creato.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.mcp.token.revoke.help = uso: { $syntax }{ $line_break }{ $line_break }Revoca il token bearer di un'etichetta.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }l'etichetta del token da revocare{ $line_break }

journal.cli.host.mcp.token.revoked.bearer.token.for.label = Token bearer revocato per l'etichetta { $label }.

journal.cli.host.mcp.token.save.this.token.now.it.will.not.be.shown.again = Salva questo token ora. Non sarà mostrato di nuovo e non può essere recuperato:

journal.cli.host.mcp.token.token.management.is.not.compiled.into.this.build = la gestione di { $command_solstone_journal_mcp_token } non è compilata in questo build

journal.cli.host.mcp.token.until.you.grant.it.read.access.an.agent.using.this = Finché non gli concedi l'accesso in lettura, un agente che usa questo token vede solo { $literal_save_memory } e { $literal_recall_memory }. Per consentirgli di leggere tutto il tuo diario (aggiungi { $literal_facet } NAME per consentire solo quella facetta):

journal.cli.host.mcp.usage = uso: { $syntax }{ $line_break }

journal.cli.host.navigate.error.is.no.longer.supported.put.facet.selection.in.the.destination.url.for = { $command }: errore: { $option } non è più supportato. Metti la selezione della facetta in { $term } di destinazione; per esempio, { $path }

journal.cli.host.navigate.help = uso: { $syntax }{ $line_break }{ $line_break }Porta il browser su un percorso.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_18 }percorso { $term } a cui navigare.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.navigate.print.navigation.navigate.path = Naviga: { $path }

journal.cli.host.navigate.run.navigate.callosum.socket.unavailable.path = solstone journal navigate: errore: socket { $literal_1 } non disponibile: { $path }

journal.cli.host.navigate.run.navigate.failed.encode.callosum.request = solstone journal navigate: errore: non è stato possibile codificare la richiesta { $literal_1 }: { $error }

journal.cli.host.navigate.usage = uso: { $syntax }{ $line_break }

journal.cli.host.open.help = uso: { $syntax }{ $line_break }{ $line_break }apri il tuo diario nel tuo browser con un link monouso.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_10 }pagina da aprire (predefinito: /){ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_4 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_3 }stampa il link monouso invece di aprire un browser{ $line_break }

journal.cli.host.open.mint.could.not.make.link = non è stato possibile creare un link per il tuo diario: { $error }

journal.cli.host.open.mint.could.not.make.link.try = non è stato possibile creare un link per il tuo diario. riprova.

journal.cli.host.open.mint.response.from.could.not.be = non è stato possibile leggere la risposta del tuo diario. riprova.

journal.cli.host.open.run.browser.could.not.be.opened = non è stato possibile aprire un browser qui. esegui di nuovo questo comando con { $literal_1 } e apri il link che stampa entro un minuto.

journal.cli.host.open.run.open.it.from.another.computer = per aprirlo da un altro computer, prima inoltra le porte del diario tramite { $literal_1 } e tieni aperto quel terminale:

journal.cli.host.open.run.open.url.path = apri il tuo diario: { $url }{ $path }

journal.cli.host.open.socket.linux.down.isn.t.running.for.this = il tuo diario non è in esecuzione per questo account su questo computer, quindi non c'è niente da aprire. avvialo con { $literal_1 }.

journal.cli.host.open.socket.linux.old.running.older.than.this.command = il tuo diario in esecuzione è più vecchio di questo comando. riavvialo con { $literal_1 }, poi { $literal_2 }, e riprova.

journal.cli.host.open.socket.mac.down.isn.t.running.for.this = il tuo diario non è in esecuzione per questo account su questo computer, quindi non c'è niente da aprire. apri l'app del diario per avviarlo.

journal.cli.host.open.socket.mac.old.running.older.than.this.command = il tuo diario in esecuzione è più vecchio di questo comando. esci dall'app del diario, aprila di nuovo, e riprova.

journal.cli.host.open.socket.windows.down.isn.t.running.for.this = il tuo diario non è in esecuzione per questo account su questo computer, quindi non c'è niente da aprire. avvialo dall'app del diario e riprova.

journal.cli.host.open.socket.windows.old.running.older.than.this.command = il tuo diario in esecuzione è più vecchio di questo comando. riavvialo dall'app del diario, e riprova.

journal.cli.host.open.usage = uso: { $syntax }{ $line_break }

journal.cli.host.parser.convey.argument.journal.expected.one.argument = { $command }: errore: argomento { $flag }: atteso un solo argomento

journal.cli.host.parser.convey.argument.port.expected.one.argument = { $command }: errore: argomento { $flag }: atteso un solo argomento

journal.cli.host.parser.convey.argument.port.invalid.int.value = { $command }: errore: argomento { $flag }: valore intero non valido

journal.cli.host.parser.convey.argument.port.invalid.int.value.value = { $command }: errore: argomento { $flag }: valore intero non valido: '{ $value }'

journal.cli.host.parser.convey.argument.port.must.be.between.n1.and.n65535 = { $command }: errore: argomento { $flag }: deve essere compreso tra 1 e 65535

journal.cli.host.parser.convey.the.following.arguments.are.required.port = { $command }: errore: i seguenti argomenti sono obbligatori: { $flag }

journal.cli.host.parser.convey.unrecognized.arguments.arguments = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.host.parser.cortex.unrecognized.arguments.arguments = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.host.parser.direct.port.value.argument.direct.port.invalid.int.value.value = { $command }: errore: argomento { $flag }: valore intero non valido: '{ $value }'

journal.cli.host.parser.direct.port.value.argument.direct.port.must.be.between.n1.and.n65535 = { $command }: errore: argomento { $flag }: deve essere compreso tra 1 e 65535

journal.cli.host.parser.grab.argument.out.expected.one.argument = { $command }: errore: argomento { $flag }: atteso un solo argomento

journal.cli.host.parser.grab.path.tokens.must.be.valid.unicode = { $command }: errore: i token di percorso devono essere Unicode validi

journal.cli.host.parser.grab.unrecognized.arguments.arguments = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.host.parser.mcp.activity.expected.argument.after.name = { $command }: errore: argomento atteso dopo { $name }

journal.cli.host.parser.mcp.activity.limit.must.be.between.n1.and.n1000 = { $command }: errore: { $flag } deve essere compreso tra 1 e 1000

journal.cli.host.parser.mcp.activity.must.be.nonempty.name = { $command }: errore: { $name } deve essere non vuoto

journal.cli.host.parser.mcp.activity.unknown.option.accepted.token.oauth.created.tool.outcome.day.from.day.to.limit.json.other = { $command }: errore: opzione sconosciuta { $other }; accettati: { $flag }, { $flag_2 }, { $flag_3 }, { $flag_4 }, { $flag_5 }, { $flag_6 }, { $flag_7 }, { $flag_8 }, { $flag_9 }

journal.cli.host.parser.mcp.invalid.mcp.command.value = { $command }: errore: comando { $term } non valido: { $value }

journal.cli.host.parser.mcp.oauth.client.id.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: errore: client-id deve essere { $term } valido e non vuoto

journal.cli.host.parser.mcp.oauth.client.id.expected.exactly.one.client.id.client.id.argument = { $command }: errore: atteso esattamente un argomento { $flag } { $argument }

journal.cli.host.parser.mcp.oauth.client.id.the.following.arguments.are.required.client.id.client.id = { $command }: errore: i seguenti argomenti sono obbligatori: { $flag } { $argument }

journal.cli.host.parser.mcp.oauth.invalid.mcp.oauth.command.value = { $command }: errore: comando oauth { $term } non valido: { $value }

journal.cli.host.parser.mcp.oauth.the.following.arguments.are.required.list.revoke = { $command }: errore: i seguenti argomenti sono obbligatori: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.expected.optional.door.local.lan.solstone.me.hostname.argument = { $command }: errore: atteso argomento facoltativo { $syntax }

journal.cli.host.parser.mcp.pairing.invalid.mcp.pairing.command.value = { $command }: errore: comando di accoppiamento { $term } non valido: { $value }

journal.cli.host.parser.mcp.pairing.the.following.arguments.are.required.generate.revoke = { $command }: errore: i seguenti argomenti sono obbligatori: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.unknown.door.choose.one.of.local.lan.solstone.me.hostname = { $command }: errore: porta sconosciuta, scegli una tra { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }

journal.cli.host.parser.mcp.permission.invalid.mcp.permission.command.value = { $command }: errore: comando di permesso { $term } non valido: { $value }

journal.cli.host.parser.mcp.permission.set.at.least.one.category.is.required = { $command }: errore: almeno un { $flag } è obbligatorio

journal.cli.host.parser.mcp.permission.set.category.must.be.nonempty = { $command }: errore: la categoria deve essere non vuota

journal.cli.host.parser.mcp.permission.set.category.must.be.valid.utf.n8.and.nonempty = { $command }: errore: la categoria deve essere { $term } valido e non vuota

journal.cli.host.parser.mcp.permission.set.expected.argument.after.category = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.permission.set.expected.argument.after.facet = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.permission.set.expected.argument.after.places.precision = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.nonempty = { $command }: errore: il nome della facetta deve essere non vuoto

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.valid.utf.n8.and.nonempty = { $command }: errore: il nome della facetta deve essere { $term } valido e non vuoto

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.nonempty = { $command }: errore: la precisione dei luoghi deve essere non vuota

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.valid.utf.n8.and.nonempty = { $command }: errore: la precisione dei luoghi deve essere { $term } valido e non vuota

journal.cli.host.parser.mcp.permission.set.places.precision.repeated = { $command }: errore: { $flag } può essere indicato una sola volta

journal.cli.host.parser.mcp.permission.the.following.arguments.are.required.show.set.clear = { $command }: errore: i seguenti argomenti sono obbligatori: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.mcp.probe.arguments.is.required = { $command }: errore: { $flag } è obbligatorio

journal.cli.host.parser.mcp.probe.expected.argument.after.arguments = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.probe.expected.argument.after.tool = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.probe.tool.is.required = { $command }: errore: { $flag } è obbligatorio

journal.cli.host.parser.mcp.target.created.cannot.be.used.with.token = { $command }: errore: { $flag } non può essere usato con { $flag_2 }

journal.cli.host.parser.mcp.target.created.timestamp.must.be.nonempty = { $command }: errore: il timestamp di creazione deve essere non vuoto

journal.cli.host.parser.mcp.target.created.timestamp.must.be.valid.utf.n8.and.nonempty = { $command }: errore: il timestamp di creazione deve essere { $term } valido e non vuoto

journal.cli.host.parser.mcp.target.expected.argument.after.created = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.target.expected.argument.after.oauth = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.target.expected.argument.after.token = { $command }: errore: argomento atteso dopo { $flag }

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.nonempty = { $command }: errore: oauth client-id deve essere non vuoto

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: errore: oauth client-id deve essere { $term } valido e non vuoto

journal.cli.host.parser.mcp.target.one.of.token.or.oauth.is.required = { $command }: errore: è obbligatorio uno tra { $flag } o { $flag_2 }

journal.cli.host.parser.mcp.target.specify.either.token.or.oauth.not.both = { $command }: errore: specifica { $flag } oppure { $flag_2 }, non entrambi

journal.cli.host.parser.mcp.target.token.label.must.be.nonempty = { $command }: errore: l'etichetta del token deve essere non vuota

journal.cli.host.parser.mcp.target.token.label.must.be.valid.utf.n8.and.nonempty = { $command }: errore: l'etichetta del token deve essere { $term } valido e non vuota

journal.cli.host.parser.mcp.target.unexpected.argument.option = { $command }: errore: argomento imprevisto: { $option }

journal.cli.host.parser.mcp.the.following.arguments.are.required.service.local.door.status.token.pairing.oauth.permission.activity.probe = { $command }: errore: i seguenti argomenti sono obbligatori: { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }, { $literal_5 }, { $literal_6 }, { $literal_7 }, { $literal_8 }, { $literal_9 }

journal.cli.host.parser.mcp.token.invalid.mcp.token.command.value = { $command }: errore: comando token { $term } non valido: { $value }

journal.cli.host.parser.mcp.token.label.expected.exactly.one.label.label.argument = { $command }: errore: atteso esattamente un argomento { $flag } { $argument }

journal.cli.host.parser.mcp.token.label.label.must.be.valid.utf.n8.and.nonempty = { $command }: errore: l'etichetta deve essere { $term } valido e non vuota

journal.cli.host.parser.mcp.token.label.the.following.arguments.are.required.label.label = { $command }: errore: i seguenti argomenti sono obbligatori: { $flag } { $argument }

journal.cli.host.parser.mcp.token.the.following.arguments.are.required.create.list.revoke = { $command }: errore: i seguenti argomenti sono obbligatori: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.schedule.unrecognized.arguments.arguments = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.host.parser.spl.invalid.choice.choose.from.service.value = { $command }: errore: scelta non valida: { $value } (scegli tra { $literal })

journal.cli.host.parser.spl.the.following.arguments.are.required.service = { $command }: errore: i seguenti argomenti sono obbligatori: { $literal }

journal.cli.host.parser.spl.unrecognized.arguments.arguments = { $command }: errore: argomenti non riconosciuti: { $arguments }

journal.cli.host.parser.supervisor.argument.direct.port.cannot.be.repeated = { $command }: errore: argomento { $flag }: non può essere ripetuto

journal.cli.host.parser.supervisor.argument.direct.port.expected.one.argument = { $command }: errore: argomento { $flag }: atteso un solo argomento

journal.cli.host.parser.supervisor.argument.direct.port.invalid.int.value = { $command }: errore: argomento { $flag }: valore intero non valido

journal.cli.host.parser.validate.day.day.must.be.eight.digits.yyyymmdd.got.value = { $command }: errore: il giorno deve essere di otto cifre ({ $argument }), ricevuto { $value }

journal.cli.host.process.admission = ammissione del processo del diario non riuscita: { $error }

journal.cli.host.read.error.error.solstone.observe.utils.error.reading = { $literal_error_solstone_observe_utils }:Errore durante la lettura di { $path }: { $error }

journal.cli.host.render.oauth.store.error.no.active.pairing.code = { $command_solstone_journal_mcp } { $command } { $operation }: nessun codice di accoppiamento attivo

journal.cli.host.render.oauth.store.error.no.such.oauth.client = { $command_solstone_journal_mcp } { $command } { $operation }: nessun client { $literal_oauth }

journal.cli.host.render.supervisor.host.outcome.supervisor.failed.to.boot = non è stato possibile avviare il supervisore: { $reason }

journal.cli.host.render.supervisor.host.outcome.supervisor.lifecycle.shutdown.failed.cause.readiness.self.heartbeat.identity = non è stato possibile arrestare il ciclo di vita del supervisore: causa={ $cause }, prontezza={ $readiness }, self_heartbeat={ $self_heartbeat }, identità={ $identity }

journal.cli.host.render.token.store.error.token = { $command_solstone_journal_mcp_token } { $operation }: { $error }

journal.cli.host.render.token.store.error.token.a.bearer.token.already.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: esiste già un token bearer per l'etichetta { $label }

journal.cli.host.render.token.store.error.token.invalid.label = { $command_solstone_journal_mcp_token } { $operation }: etichetta non valida: { $error }

journal.cli.host.render.token.store.error.token.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: non esiste alcun token bearer per l'etichetta { $label }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.oauth.store = { $command_solstone_journal_mcp }: non è stato possibile interrogare l'archivio oauth: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.token.store = { $command_solstone_journal_mcp }: non è stato possibile interrogare l'archivio dei token: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.multiple.oauth.grants.matched.client.id.specify = { $command_solstone_journal_mcp }: più concessioni { $literal_oauth } corrispondono a { $literal_client_id } { $client_id }; specifica { $literal_created } { $timestamp_arg }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp }: non esiste alcun token bearer per l'etichetta { $label }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.oauth.grant.found.matching.client.id = { $command_solstone_journal_mcp }: nessuna concessione { $literal_oauth } trovata corrispondente a { $literal_client_id } { $client_id }

journal.cli.host.schedule.help = uso: { $syntax }{ $line_break }{ $line_break }Mostra i compiti pianificati{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Attiva la registrazione di debug{ $line_break }

journal.cli.host.schedule.usage = uso: { $syntax }{ $line_break }

journal.cli.host.sense.help = uso: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }{ $line_break }Processore unificato di file observe{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_13 }Elabora i file di un giorno specifico (formato { $argument }) invece di seguirli{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_2 }Numero massimo di compiti screen-describe simultanei quando si usa { $flag } (predefinito: 1).{ $line_break }{ $spaces_2 }{ $flag_14 } { $choices }{ $line_break }{ $spaces_24 }Elimina gli output esistenti di schermo o audio e rielabora (richiede { $flag }){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_5 }Filtra per un segmento specifico (formato { $argument_19 }, richiede { $flag }){ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_7 }Filtra per un flusso specifico (richiede { $flag }){ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_13 }Mostra ciò che verrebbe elaborato (o eliminato con { $flag_14 }) senza apportare modifiche{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_9 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_11 }Attiva la registrazione di debug{ $line_break }

journal.cli.host.sense.sense.batch.failed = lotto sense non riuscito: { $error }

journal.cli.host.sense.sense.parent.loss.admission.failed = ammissione sense per perdita del superiore non riuscita: { $error }

journal.cli.host.sense.service.sense.service.failed = servizio sense non riuscito: { $class }

journal.cli.host.sense.usage = uso: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }

journal.cli.host.service.argument.port = errore: argomento imprevisto '{ $text }'; passa la porta come { $flag } { $port }

journal.cli.host.service.argument.range = errore: argomento imprevisto '{ $text }'; passa una porta compresa tra 1 e 65535 come { $flag } { $argument }

journal.cli.host.service.argument.unexpected = errore: argomento imprevisto '{ $text }'; passa la porta come { $flag } { $argument }

journal.cli.host.service.conflicting.absent.systemd.state = stato assente di { $systemd } in conflitto

journal.cli.host.service.could.not.clear.the.service.s.ready.state = non è stato possibile azzerare lo stato pronto del servizio: { $error }

journal.cli.host.service.could.not.find.the.solstone.installation.from = non è stato possibile trovare l'installazione di { $solstone } da { $path }

journal.cli.host.service.could.not.resolve.journal = non è stato possibile risolvere il diario

journal.cli.host.service.could.not.resolve.the.solstone.executable = non è stato possibile risolvere l'eseguibile di { $solstone }: { $error }

journal.cli.host.service.could.not.verify.the.saved.installation = non è stato possibile verificare l'installazione salvata: { $error }

journal.cli.host.service.could.not.verify.the.solstone.installation = non è stato possibile verificare l'installazione di { $solstone }: { $error }

journal.cli.host.service.could.not.verify.the.solstone.setup.storage = non è stato possibile verificare l'archivio della configurazione iniziale di { $solstone }: { $error }

journal.cli.host.service.couldn.t.confirm.that.the.service.uses.this.journal = non è stato possibile confermare che il servizio usa questo diario. esegui '{ $setup }' per aggiornare la posizione salvata.

journal.cli.host.service.create.service.directory = creazione della directory del servizio: { $source }

journal.cli.host.service.create.service.health.directory = creazione della directory di stato del servizio: { $source }

journal.cli.host.service.duplicate.systemd.property = proprietà { $systemd } duplicata: { $key }

journal.cli.host.service.entry.is.not.a.regular.file = l'elemento non è un file regolare

journal.cli.host.service.error.details = errore: { $detail }

journal.cli.host.service.executable.parent = l'eseguibile del servizio non ha una directory superiore

journal.cli.host.service.file.absent = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato { $path }: assente
    [install] servizio installazione rifiutato { $path }: assente
    [operate] servizio operazione rifiutato { $path }: assente
    [restart] servizio riavvio rifiutato { $path }: assente
    [route-repair] servizio riparazione del percorso rifiutato { $path }: assente
    [status] servizio stato rifiutato { $path }: assente
    [stop] servizio arresto rifiutato { $path }: assente
   *[uninstall] servizio disinstallazione rifiutato { $path }: assente
}

journal.cli.host.service.file.foreign = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
    [install] servizio installazione rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
    [operate] servizio operazione rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
    [restart] servizio riavvio rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
    [route-repair] servizio riparazione del percorso rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
    [status] servizio stato rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
    [stop] servizio arresto rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
   *[uninstall] servizio disinstallazione rifiutato { $path }: non riconosciuto come file di servizio creato dal diario
}

journal.cli.host.service.file.managed = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato { $path }: gestito
    [install] servizio installazione rifiutato { $path }: gestito
    [operate] servizio operazione rifiutato { $path }: gestito
    [restart] servizio riavvio rifiutato { $path }: gestito
    [route-repair] servizio riparazione del percorso rifiutato { $path }: gestito
    [status] servizio stato rifiutato { $path }: gestito
    [stop] servizio arresto rifiutato { $path }: gestito
   *[uninstall] servizio disinstallazione rifiutato { $path }: gestito
}

journal.cli.host.service.file.unknown = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
    [install] servizio installazione rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
    [operate] servizio operazione rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
    [restart] servizio riavvio rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
    [route-repair] servizio riparazione del percorso rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
    [status] servizio stato rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
    [stop] servizio arresto rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
   *[uninstall] servizio disinstallazione rifiutato { $path }: non è stato possibile verificare il file di servizio: { $cause }
}

journal.cli.host.service.incomplete.systemd.property.set = insieme incompleto di proprietà { $systemd }

journal.cli.host.service.inspect.launchd.directory = ispezione della directory { $launchd }: { $error }

journal.cli.host.service.inspect.service.directory = ispezione della directory del servizio: { $error }

journal.cli.host.service.inspect.service.health.directory = ispezione della directory di stato del servizio: { $error }

journal.cli.host.service.inspect.service.journal.root = ispezione della radice del diario del servizio: { $error }

journal.cli.host.service.inspect.stale.launchd.unit = ispezione dell'unità { $launchd } non aggiornata: { $error }

journal.cli.host.service.installation.changed.while.waiting.for.the.new.journal.folder = l'installazione è cambiata durante l'attesa della nuova cartella del diario

journal.cli.host.service.launchd.accepted.the.unload.request.but.the.service.is = { $launchd } ha accettato la richiesta di rilascio, ma il servizio è ancora presente

journal.cli.host.service.launchd.directory.is.not.a.real.directory = la directory { $launchd } non è una directory reale

journal.cli.host.service.launchd.observation.was.not.utf = l'osservazione di { $launchd } non era UTF-8

journal.cli.host.service.launchd.omitted.or.malformed.arguments = { $launchd }: argomenti omessi o malformati

journal.cli.host.service.launchd.omitted.the.runtime.state = { $launchd } ha omesso lo stato dell'ambiente di esecuzione

journal.cli.host.service.malformed.systemd.property = proprietà { $systemd } malformata

journal.cli.host.service.manager.disable.service = non è stato possibile disattivare il servizio: { $result }

journal.cli.host.service.manager.disable.stale.service = non è stato possibile disattivare il servizio non aggiornato: { $result }

journal.cli.host.service.manager.enable.service = non è stato possibile attivare il servizio: { $result }

journal.cli.host.service.manager.load.installed.service = non è stato possibile caricare il servizio installato: { $result }

journal.cli.host.service.manager.load.service = non è stato possibile caricare il servizio: { $result }

journal.cli.host.service.manager.load.service.after.changing.journal.folder = non è stato possibile caricare il servizio dopo il cambio della cartella del diario: { $result }

journal.cli.host.service.manager.reload.service.after.changing.journal.folder = non è stato possibile ricaricare il servizio dopo il cambio della cartella del diario: { $result }

journal.cli.host.service.manager.reload.systemd = non è stato possibile ricaricare systemd: { $result }

journal.cli.host.service.manager.reload.systemd.for.route.repair = non è stato possibile ricaricare systemd per la riparazione del percorso: { $result }

journal.cli.host.service.manager.request.launchd.unload = non è stato possibile richiedere il rilascio a launchd: { $result }

journal.cli.host.service.manager.restart.service = non è stato possibile riavviare il servizio: { $result }

journal.cli.host.service.manager.restart.service.for.route.repair = non è stato possibile riavviare il servizio per la riparazione del percorso: { $result }

journal.cli.host.service.manager.start.after.changing.journal.folder = non è stato possibile avviare dopo il cambio della cartella del diario: { $result }

journal.cli.host.service.manager.start.service = non è stato possibile avviare il servizio: { $result }

journal.cli.host.service.manager.stop.service = non è stato possibile fermare il servizio: { $result }

journal.cli.host.service.manager.stop.service.before.changing.journal.folder = non è stato possibile fermare il servizio prima del cambio della cartella del diario: { $result }

journal.cli.host.service.manager.stop.service.before.restart = non è stato possibile fermare il servizio prima del riavvio: { $result }

journal.cli.host.service.manager.stop.stale.service = non è stato possibile fermare il servizio non aggiornato: { $result }

journal.cli.host.service.manager.unload.stale.launchd.service = non è stato possibile rilasciare il servizio launchd non aggiornato: { $result }

journal.cli.host.service.manager.unload.stale.service = non è stato possibile rilasciare il servizio non aggiornato: { $result }

journal.cli.host.service.metadata = metadati: { $error }

journal.cli.host.service.open.your.journal = apri il tuo diario: { $url }

journal.cli.host.service.open.your.journal.solstone.journal.open = apri il tuo diario: { $open }

journal.cli.host.service.parse.stale.launchd.unit = analisi dell'unità { $launchd } non aggiornata: { $error }

journal.cli.host.service.port.invalid = errore: porta non valida '{ $value }'

journal.cli.host.service.preserved.service.unit = unità del servizio preservata { $path }

journal.cli.host.service.read = lettura: { $error }

journal.cli.host.service.ready.timeout = il servizio non è diventato pronto entro 120 secondi

journal.cli.host.service.recheck.stale.launchd.unit = nuovo controllo dell'unità { $launchd } non aggiornata: { $error }

journal.cli.host.service.registration.a.different.unit.answers.to.this.name = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te la pulizia del servizio launchd non aggiornato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [install] servizio installazione rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'installazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [operate] servizio operazione rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'operazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'avvio dopo il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [status] servizio stato rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te lo stato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [stop] servizio arresto rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'arresto -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [uninstall] servizio disinstallazione rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te la disinstallazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [up] servizio messa in funzione rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te la messa in funzione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [verify-service-stop] servizio verifica dell'arresto del servizio rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica dell'arresto del servizio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
   *[verify-unload] servizio verifica del rilascio rifiutato: un'unità diversa risponde a questo nome. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica del rilascio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.launch.agent.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la pulizia del servizio launchd non aggiornato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [install] servizio installazione rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'installazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [operate] servizio operazione rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'operazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'avvio dopo il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [status] servizio stato rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te lo stato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [stop] servizio arresto rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'arresto -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [uninstall] servizio disinstallazione rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la disinstallazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [up] servizio messa in funzione rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la messa in funzione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [verify-service-stop] servizio verifica dell'arresto del servizio rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica dell'arresto del servizio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
   *[verify-unload] servizio verifica del rilascio rifiutato: l'agente di avvio non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica del rilascio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.launch.agent.s.program.is.not.a.journal = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la pulizia del servizio launchd non aggiornato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [install] servizio installazione rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'installazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [operate] servizio operazione rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'operazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'avvio dopo il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [status] servizio stato rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te lo stato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [stop] servizio arresto rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'arresto -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [uninstall] servizio disinstallazione rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la disinstallazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [up] servizio messa in funzione rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la messa in funzione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [verify-service-stop] servizio verifica dell'arresto del servizio rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica dell'arresto del servizio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
   *[verify-unload] servizio verifica del rilascio rifiutato: il programma dell'agente di avvio non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica del rilascio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.unit.carries.a.drop.in.journal.did.not = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la pulizia del servizio launchd non aggiornato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [install] servizio installazione rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'installazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [operate] servizio operazione rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'operazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'avvio dopo il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [status] servizio stato rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te lo stato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [stop] servizio arresto rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'arresto -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [uninstall] servizio disinstallazione rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la disinstallazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [up] servizio messa in funzione rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la messa in funzione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [verify-service-stop] servizio verifica dell'arresto del servizio rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica dell'arresto del servizio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
   *[verify-unload] servizio verifica del rilascio rifiutato: l'unità ha un drop-in che il diario non ha scritto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica del rilascio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.unit.file.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la pulizia del servizio launchd non aggiornato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [install] servizio installazione rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'installazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [operate] servizio operazione rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'operazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'avvio dopo il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [status] servizio stato rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te lo stato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [stop] servizio arresto rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'arresto -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [uninstall] servizio disinstallazione rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la disinstallazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [up] servizio messa in funzione rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la messa in funzione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [verify-service-stop] servizio verifica dell'arresto del servizio rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica dell'arresto del servizio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
   *[verify-unload] servizio verifica del rilascio rifiutato: il file dell'unità non è quello gestito dal diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica del rilascio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.unit.is.in.an.unexpected.state = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la pulizia del servizio launchd non aggiornato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [install] servizio installazione rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'installazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [operate] servizio operazione rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'operazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'avvio dopo il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [status] servizio stato rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te lo stato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [stop] servizio arresto rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'arresto -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [uninstall] servizio disinstallazione rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la disinstallazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [up] servizio messa in funzione rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la messa in funzione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [verify-service-stop] servizio verifica dell'arresto del servizio rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica dell'arresto del servizio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
   *[verify-unload] servizio verifica del rilascio rifiutato: l'unità è in uno stato imprevisto. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica del rilascio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.unit.s.execstart.is.not.a.journal.launcher = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la pulizia del servizio launchd non aggiornato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [install] servizio installazione rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'installazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [operate] servizio operazione rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'operazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'avvio dopo il cambio della cartella del diario -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [status] servizio stato rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te lo stato -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [stop] servizio arresto rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te l'arresto -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [uninstall] servizio disinstallazione rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la disinstallazione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [up] servizio messa in funzione rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la messa in funzione -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
    [verify-service-stop] servizio verifica dell'arresto del servizio rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica dell'arresto del servizio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
   *[verify-unload] servizio verifica del rilascio rifiutato: ExecStart dell'unità non è un programma di avvio del diario. il diario non lo sovrascriverà. Se è intenzionale, completa da te la verifica del rilascio -- su Linux: `{ $restart_command }` -- poi conferma la build in esecuzione con `{ $inspect_command }`
}

journal.cli.host.service.registration.unknown = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario: non è stato possibile controllare la registrazione del servizio: { $cause }
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato: non è stato possibile controllare la registrazione del servizio: { $cause }
    [install] servizio installazione: non è stato possibile controllare la registrazione del servizio: { $cause }
    [operate] servizio operazione: non è stato possibile controllare la registrazione del servizio: { $cause }
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario: non è stato possibile controllare la registrazione del servizio: { $cause }
    [status] servizio stato: non è stato possibile controllare la registrazione del servizio: { $cause }
    [stop] servizio arresto: non è stato possibile controllare la registrazione del servizio: { $cause }
    [uninstall] servizio disinstallazione: non è stato possibile controllare la registrazione del servizio: { $cause }
    [up] servizio messa in funzione: non è stato possibile controllare la registrazione del servizio: { $cause }
    [verify-service-stop] servizio verifica dell'arresto del servizio: non è stato possibile controllare la registrazione del servizio: { $cause }
   *[verify-unload] servizio verifica del rilascio: non è stato possibile controllare la registrazione del servizio: { $cause }
}

journal.cli.host.service.remove.service.unit = rimozione dell'unità del servizio: { $error }

journal.cli.host.service.remove.stale.launchd.unit = rimozione dell'unità { $launchd } non aggiornata: { $error }

journal.cli.host.service.removed = rimosso { $path }

journal.cli.host.service.removed.stale.launchd.plist = rimosso il plist { $launchd } non aggiornato { $path }

journal.cli.host.service.removed.stale.service.registration = rimossa la registrazione non aggiornata del servizio

journal.cli.host.service.route.repair.service.launcher.is.not.an.executable.regular = il programma di avvio del servizio di riparazione del percorso non è un file regolare eseguibile

journal.cli.host.service.route.repair.service.launcher.is.unavailable = il programma di avvio del servizio di riparazione del percorso non è disponibile: { $error }

journal.cli.host.service.route.repair.service.unit.has.an.invalid.command.tail = l'unità del servizio di riparazione del percorso ha una coda di comando non valida

journal.cli.host.service.route.repair.service.unit.has.an.invalid.execstart = l'unità del servizio di riparazione del percorso ha un { $exec_start } non valido

journal.cli.host.service.route.repair.service.unit.has.no.execstart = l'unità del servizio di riparazione del percorso non ha { $exec_start }

journal.cli.host.service.route.repair.service.unit.is.not.utf = l'unità del servizio di riparazione del percorso non è UTF-8

journal.cli.host.service.run.solstone.journal.setup.or.solstone.journal.service.install = esegui '{ $setup }' o '{ $install }' per installarlo.

journal.cli.host.service.scan.launchd.directory = scansione della directory { $launchd }: { $error }

journal.cli.host.service.scan.launchd.entry = scansione dell'elemento { $launchd }: { $error }

journal.cli.host.service.service.became.active.during.the.journal.folder.change = il servizio è diventato attivo durante il cambio della cartella del diario

journal.cli.host.service.service.command.output.exceeded.kib = l'output del comando del servizio ha superato 256 KiB

journal.cli.host.service.service.command.output.failed = non è stato possibile ottenere l'output del comando del servizio: { $error }

journal.cli.host.service.service.command.spawn.failed = non è stato possibile avviare il processo del comando del servizio: { $error }

journal.cli.host.service.service.command.timed.out = il comando del servizio è scaduto

journal.cli.host.service.service.command.wait.failed = non è stato possibile attendere il comando del servizio: { $error }

journal.cli.host.service.service.directory.contains.unsafe.component = la directory del servizio contiene un componente non sicuro

journal.cli.host.service.service.directory.escaped.home = la directory del servizio è uscita dalla directory personale

journal.cli.host.service.service.enabled = servizio attivato

journal.cli.host.service.service.health.path.is.not.a.real.directory = il percorso di stato del servizio non è una directory reale

journal.cli.host.service.service.home = directory personale del servizio: { $error }

journal.cli.host.service.service.install.could.not.resolve.journal = servizio installazione: non è stato possibile risolvere il diario

journal.cli.host.service.service.install.current.executable.has.no.directory = servizio installazione: l'eseguibile attuale non ha una directory

journal.cli.host.service.service.install.invalid.installation.identity.arguments = servizio installazione: argomenti di identità dell'installazione non validi: { $error }

journal.cli.host.service.service.install.requires.installation.identity.arguments = l'installazione del servizio richiede argomenti di identità dell'installazione

journal.cli.host.service.service.installed = servizio: installato

journal.cli.host.service.service.journal.root.is.not.a.real.directory = la radice del diario del servizio non è una directory reale

journal.cli.host.service.service.launcher.is.not.an.executable.regular.file = il programma di avvio del servizio non è un file regolare eseguibile

journal.cli.host.service.service.launcher.is.unavailable = il programma di avvio del servizio non è disponibile: { $error }

journal.cli.host.service.service.loaded.into.launchd = servizio caricato in { $launchd }

journal.cli.host.service.service.lock = blocco del servizio: { $error }

journal.cli.host.service.service.management.is.supported.only.on.linux.and.mac = la gestione del servizio è supportata solo su linux e mac

journal.cli.host.service.service.not.installed = servizio: non installato

journal.cli.host.service.service.not.installed.run.solstone.journal.service.install.first = servizio non installato. esegui '{ $install }' prima.

journal.cli.host.service.service.path.is.not.utf = il percorso del servizio non è UTF-8

journal.cli.host.service.service.registration.has.no.matching.managed.file = la registrazione del servizio non ha un file gestito corrispondente

journal.cli.host.service.service.registration.has.no.valid.port = la registrazione del servizio non ha una porta valida

journal.cli.host.service.service.restarted = servizio riavviato

journal.cli.host.service.service.runtime.state.was.unexpected = { $operation ->
    [change-journal-folder] servizio cambio della cartella del diario: lo stato dell'ambiente di esecuzione era imprevisto
    [clean-stale-launchd-service] servizio pulizia del servizio launchd non aggiornato: lo stato dell'ambiente di esecuzione era imprevisto
    [install] servizio installazione: lo stato dell'ambiente di esecuzione era imprevisto
    [operate] servizio operazione: lo stato dell'ambiente di esecuzione era imprevisto
    [start-after-changing-journal-folder] servizio avvio dopo il cambio della cartella del diario: lo stato dell'ambiente di esecuzione era imprevisto
    [status] servizio stato: lo stato dell'ambiente di esecuzione era imprevisto
    [stop] servizio arresto: lo stato dell'ambiente di esecuzione era imprevisto
    [uninstall] servizio disinstallazione: lo stato dell'ambiente di esecuzione era imprevisto
    [up] servizio messa in funzione: lo stato dell'ambiente di esecuzione era imprevisto
    [verify-service-stop] servizio verifica dell'arresto del servizio: lo stato dell'ambiente di esecuzione era imprevisto
   *[verify-unload] servizio verifica del rilascio: lo stato dell'ambiente di esecuzione era imprevisto
}

journal.cli.host.service.service.started = servizio avviato

journal.cli.host.service.service.stderr.reader.panicked = il lettore di stderr del servizio è andato in panic

journal.cli.host.service.service.stderr.unavailable = stderr del servizio non disponibile

journal.cli.host.service.service.stdout.reader.panicked = il lettore di stdout del servizio è andato in panic

journal.cli.host.service.service.stdout.unavailable = stdout del servizio non disponibile

journal.cli.host.service.service.stopped = servizio fermato

journal.cli.host.service.service.stopped.but.its.ready.state.could.not.be = servizio fermato, ma non è stato possibile azzerare il suo stato pronto: { $error }

journal.cli.host.service.service.unit.changed.before.publication.nothing.was.written = l'unità del servizio è cambiata prima della pubblicazione; non è stato scritto niente

journal.cli.host.service.service.unit.changed.during.operation.preserved.replacement = l'unità del servizio è cambiata durante l'operazione; sostituzione preservata

journal.cli.host.service.service.unit.has.no.parent = l'unità del servizio non ha una directory superiore

journal.cli.host.service.service.unit.has.no.trusted.home.ancestor = l'unità del servizio non ha un antenato attendibile della directory personale

journal.cli.host.service.service.unit.publication.failed = non è stato possibile pubblicare l'unità del servizio: { $error }

journal.cli.host.service.service.unit.published.with.uncertain.durability = unità del servizio pubblicata con durabilità incerta: { $other }

journal.cli.host.service.service.was.not.installed = il servizio non era installato

journal.cli.host.service.sign.in.invalid = errore: { $flag } si aspetta { $literal } o { $literal_3 }

journal.cli.host.service.stale.launchd.plist.changed.during.cleanup.preserved = il plist { $launchd } non aggiornato è cambiato durante la pulizia; preservato { $path }

journal.cli.host.service.stale.launchd.unit.has.a.foreign.label = l'unità { $launchd } non aggiornata ha un'etichetta estranea

journal.cli.host.service.stale.launchd.unit.has.conflicting.program.fields = l'unità { $launchd } non aggiornata ha campi del programma in conflitto

journal.cli.host.service.stale.launchd.unit.has.no.label = l'unità { $launchd } non aggiornata non ha un'etichetta

journal.cli.host.service.stale.launchd.unit.has.no.launcher = l'unità { $launchd } non aggiornata non ha un programma di avvio

journal.cli.host.service.stale.launchd.unit.is.foreign.or.unrecognized = l'unità { $launchd } non aggiornata è estranea o non riconosciuta

journal.cli.host.service.stale.launchd.unit.is.not.a.dictionary = l'unità { $launchd } non aggiornata non è un dizionario

journal.cli.host.service.state.running = stato: in esecuzione ({ $manager })

journal.cli.host.service.state.stopped = stato: fermato

journal.cli.host.service.subcommand.unknown = sottocomando sconosciuto: { $value }; disponibili: { $literal }

journal.cli.host.service.systemd.observation.was.not.utf = l'osservazione di { $systemd } non era UTF-8

journal.cli.host.service.the.previous.service.did.not.stop.before.replacement = il servizio precedente non si è fermato prima della sostituzione

journal.cli.host.service.the.solstone.executable.has.no.containing.directory = l'eseguibile di { $solstone } non ha una directory superiore

journal.cli.host.service.unit.exceeds.mib = l'unità supera 1 MiB

journal.cli.host.service.unit.path.has.no.trusted.home.ancestor = il percorso dell'unità non ha un antenato che sia una directory personale attendibile

journal.cli.host.service.unrecognized.systemd.active.substate.pair = coppia active/substate di { $systemd } non riconosciuta

journal.cli.host.service.unsafe.service.directory = directory del servizio non sicura: { $path }

journal.cli.host.service.usage = Uso: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $spaces_2 }(predefinito: 5015; solo { $term }){ $line_break }{ $spaces_7 }{ $syntax_7 }{ $spaces_2 }(riavvio; { $flag } non fa nulla se non è installato){ $line_break }{ $spaces_7 }{ $syntax_9 }{ $spaces_29 }(avvio + stato; il servizio deve essere installato){ $line_break }{ $spaces_7 }{ $syntax_11 }{ $spaces_27 }(arresto){ $line_break }

journal.cli.host.service.windows.only = questo comando di servizio è disponibile solo su Windows

journal.cli.host.service.wrote = scritto { $path }

journal.cli.host.settings.bind = convey{ $line_break }{ $space_2 }riserva:{ $space_14 }{ $address }

journal.cli.host.settings.convey.error.argument.convey.command.invalid.choice.choose.from.status = { $command }: errore: argomento { $literal }: scelta non valida: '{ $value }' (scegli tra { $literal_3 })

journal.cli.host.settings.convey.help = uso: { $syntax }{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_4 }Mostra lo stato della riserva di convey e { $term } del pannello{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.settings.convey.usage = uso: { $syntax }{ $line_break }

journal.cli.host.settings.error.argument.section.invalid.choice.choose.from.convey = { $command }: errore: argomento { $literal }: scelta non valida: '{ $value }' (scegli tra { $literal_3 })

journal.cli.host.settings.error.unrecognized.arguments = { $command }: errore: argomenti non riconosciuti: { $value }

journal.cli.host.settings.help = uso: { $syntax }{ $line_break }{ $line_break }Gestisci le impostazioni locali del diario{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_7 }Gestisci le impostazioni di convey{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_2 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }Attiva il registro di debug{ $line_break }

journal.cli.host.settings.open = aprilo con:{ $space_6 }{ $command }

journal.cli.host.settings.port.read = Errore: { $path }: { $detail }

journal.cli.host.settings.status.help = uso: { $syntax }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }Stampa lo stato leggibile dalla macchina.{ $line_break }

journal.cli.host.settings.url = url del pannello:{ $space_5 }{ $url }

journal.cli.host.settings.usage = uso: { $syntax }{ $line_break }

journal.cli.host.setup.error = { $literal }: errore: { $detail }

journal.cli.host.setup.usage = uso: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $spaces_21 }{ $syntax_8 }{ $line_break }{ $spaces_21 }{ $syntax_9 } (solo { $term }){ $line_break }

journal.cli.host.spl.help = uso: { $syntax }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Attiva il registro di debug{ $line_break }

journal.cli.host.spl.service.spl.service.failed = servizio spl non riuscito: { $class }

journal.cli.host.spl.usage = uso: { $syntax }{ $line_break }

journal.cli.host.start.help = uso: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $line_break }Segui lo stato del sistema del diario{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }porta di { $term } (0 = selezione automatica della porta disponibile){ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Disattiva l'esecuzione di elaborazione giornaliera a mezzanotte{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }Non avviare il server { $term_17 } (eseguilo manualmente per{ $line_break }{ $spaces_21 }il debug){ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_11 }Non avviare il relay della rete privata{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_8 }Non avviare l'applicazione web di { $term }{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_6 }Non inizializzare né eseguire il pianificatore{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_4 }Richiedi che il processo superiore diretto resti in esecuzione{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_2 }Usa questo percorso come radice del diario invece della risoluzione{ $line_break }{ $spaces_21 }normale del diario.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Porta door del dispositivo accoppiato (predefinito: 7657){ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_6 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_29 }{ $spaces_8 }Attiva il registro di debug{ $line_break }

journal.cli.host.start.usage = uso: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.admission.wait.active = un altro avvio di { $brand } è in attesa dopo aver trovato attività recente del diario.{ $break_line }questo avvio non è proseguito mentre quell'attesa era attiva.{ $break_line }aspetta un momento, poi riprova.

journal.cli.host.supervisor.admission.wait.terminal = è presente un battito recente di un'altra esecuzione.{ $break_line }l'app di solstone non si è avviata mentre quel battito era ancora presente.{ $break_line }aspetta un momento, poi riprova.

journal.cli.host.supervisor.admission.wait.unverifiable = non è stato possibile verificare lo stato di avvio.{ $break_line }aspetta un momento, poi riprova.

journal.cli.host.supervisor.already.running = il tuo diario è già in esecuzione.{ $break_line }{ $break_line }questo avvio si è fatto da parte così quello in esecuzione non viene disturbato. non c'è niente da fare.{ $break_line }

journal.cli.host.supervisor.binding.home = directory personale: { $error }

journal.cli.host.supervisor.binding.installation.root = radice dell'installazione: { $error }

journal.cli.host.supervisor.binding.journal.token = token del diario: { $error }

journal.cli.host.supervisor.binding.owner.storage = archiviazione del titolare: { $error }

journal.cli.host.supervisor.binding.root.token = token root: { $error }

journal.cli.host.supervisor.binding.saved.binding = vincolo salvato: { $error }

journal.cli.host.supervisor.binding.unreadable = non è stato possibile caricare il vincolo di installazione salvato

journal.cli.host.supervisor.help = uso: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }{ $line_break }Segui lo stato del sistema del diario{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }porta di { $term } (0 = selezione automatica della porta disponibile){ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Disattiva l'esecuzione di elaborazione giornaliera a mezzanotte{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }Non avviare il server { $term_17 } (eseguilo manualmente per{ $line_break }{ $spaces_21 }il debug){ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }Non avviare il relay della rete privata{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_8 }Non avviare l'applicazione web di { $term }{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_6 }Non inizializzare né eseguire il pianificatore{ $line_break }{ $spaces_2 }{ $option_24 }{ $spaces_2 }Usa questo percorso come radice del diario invece della risoluzione{ $line_break }{ $spaces_21 }normale del diario.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Porta door del dispositivo accoppiato (predefinito: 7657){ $line_break }{ $spaces_2 }{ $option_27 }{ $spaces_6 }Attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_8 }Attiva il registro di debug{ $line_break }

journal.cli.host.supervisor.journal.program.folder = Reinstallare o disinstallare rimuove questa cartella del programma. Scegli una posizione del diario al di fuori di essa. Se il tuo diario è già lì, esci dall'app del diario. Se la configurazione iniziale è completata, esegui `{ $stop }`, poi `{ $uninstall }` e attendi che riesca. Sposta l'intera cartella del diario fuori dalla cartella del programma prima di reinstallare o disinstallare, poi seleziona la nuova posizione con `{ $setup }`, sostituendo { $path } con la nuova posizione.

journal.cli.host.supervisor.journal.token.mismatch = non è stato possibile verificare questa installazione.{ $line_break }esegui `{ $setup }` per controllarla. se la configurazione iniziale si conclude correttamente, riprova.{ $line_break }dettagli: il vincolo di installazione salvato è per un diario diverso

journal.cli.host.supervisor.lifecycle.redirect = { $supervisor } è il comando di avvio del server (prende una porta). Per il ciclo di vita, usa: { $service_verb }. Intendevi: { $requested }?

journal.cli.host.supervisor.recovery.closing = non è stato possibile continuare questo avvio.{ $line_break }{ $line_break }l'ultima esecuzione si sta ancora chiudendo. aspetta un momento, poi riprova.{ $line_break }{ $line_break }i tuoi ricordi non sono stati toccati.{ $line_break }

journal.cli.host.supervisor.recovery.coordinator.retirement.unverified = dettagli: non è stato possibile confermare l'arresto di un processo ausiliario

journal.cli.host.supervisor.recovery.coordinator.stopped.before.ready = dettagli: un processo ausiliario si è fermato prima di essere pronto

journal.cli.host.supervisor.recovery.identity.establishment = dettagli: non è stato possibile identificare un processo ausiliario

journal.cli.host.supervisor.recovery.initial.admission.handshake = dettagli: un processo ausiliario non ha risposto in tempo

journal.cli.host.supervisor.recovery.launch = dettagli: non è stato possibile avviare un processo ausiliario

journal.cli.host.supervisor.recovery.retry = non è stato possibile continuare questo avvio.{ $line_break }{ $line_break }aspetta un momento, poi riprova.{ $line_break }{ $line_break }i tuoi ricordi non sono stati toccati.{ $line_break }

journal.cli.host.supervisor.supervisor.runtime.unavailable = ambiente di esecuzione del supervisor non disponibile: { $error }

journal.cli.host.supervisor.supervisor.service.capture.unavailable = lettura del servizio del supervisor non disponibile: { $error }

journal.cli.host.supervisor.usage = uso: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.your.journal.can.t.start.because.another.account.on.this = il tuo diario non può avviarsi perché un altro account su questo computer ha il suo diario in esecuzione. su un computer è in esecuzione un solo diario alla volta, quindi il tuo può avviarsi quando quell'account ferma il suo diario o chiude la sessione.

journal.cli.host.supervisor.your.journal.can.t.start.because.port.on.this.computer = il tuo diario non può avviarsi perché la porta { $port } su questo computer è già in uso, di solito da un diario in esecuzione sotto un altro account. su questo computer può essere in esecuzione un solo diario alla volta.

journal.cli.host.thinking.chatgpt.help = uso: { $syntax }{ $line_break }{ $line_break }Gestisci l'accesso a { $term }, i token e il catalogo dei modelli.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_14 }Accedi a { $term } tramite il browser con { $term_9 }{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_13 }Esci e revoca le credenziali attive di { $term }{ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_15 }Mostra lo stato di autenticazione attuale di { $term }{ $line_break }{ $spaces_4 }{ $subcommand_14 }{ $spaces_15 }Elenca i modelli disponibili di { $term }{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.thinking.chatgpt.models.help = uso: { $syntax }{ $line_break }{ $line_break }Elenca i modelli disponibili di { $term }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Radice del diario{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Emetti { $term_11 } leggibile dalla macchina{ $line_break }

journal.cli.host.thinking.chatgpt.models.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.help = uso: { $syntax }{ $line_break }{ $line_break }Accedi a { $term } tramite il browser con { $term_4 }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_8 }Radice del diario{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }Non aprire automaticamente il browser predefinito{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.help = uso: { $syntax }{ $line_break }{ $line_break }Esci e revoca le credenziali attive di { $term }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Radice del diario{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_14 }Rimuovi il file della credenziale salvata e la registrazione{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.status.help = uso: { $syntax }{ $line_break }{ $line_break }Mostra lo stato di autenticazione attuale di { $term }.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Radice del diario{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Emetti { $term_11 } leggibile dalla macchina{ $line_break }

journal.cli.host.thinking.chatgpt.status.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.credential.chatgpt.auth.server.temporarily.unavailable = il server di autenticazione di { $literal_1 } è temporaneamente non disponibile (429 o 5xx)

journal.cli.host.thinking.credential.chatgpt.credential.store.busy = l'archivio delle credenziali di { $literal_1 } è occupato

journal.cli.host.thinking.credential.chatgpt.request.was.by.auth = la richiesta di { $literal_1 } è stata rifiutata dal server di autenticazione

journal.cli.host.thinking.credential.chatgpt.request.was.code = la richiesta di { $literal_1 } è stata rifiutata: { $code }

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload = il server di { $literal_1 } ha restituito un payload malformato ({ $field } mancante o non valido)

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload.server.returned.malformed.payload = il server di { $literal_1 } ha restituito un payload malformato

journal.cli.host.thinking.credential.chatgpt.token.grant.was.received = la concessione del token di { $literal_1 } è stata ricevuta dal server ma non è stato possibile salvarla sul disco

journal.cli.host.thinking.credential.i.o.during.credential.operation = Errore di I/O durante l'operazione sulla credenziale: { $msg }

journal.cli.host.thinking.credential.network.communicating.with.chatgpt.auth = errore di rete nella comunicazione con i server di autenticazione di { $literal_1 }

journal.cli.host.thinking.credential.path.credential.storage.unreadable.or = { $path }: l'archivio delle credenziali è illeggibile o danneggiato; esegui '{ $literal_1 }'

journal.cli.host.thinking.credential.this.chatgpt.account.isn.t = questo account { $literal_1 } non è idoneo

journal.cli.host.thinking.help = uso: { $syntax }{ $line_break }{ $line_break }Seleziona la corsia di ragionamento del diario o gestisci l'autenticazione di { $term }.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_13 }Imposta la corsia di ragionamento ({ $literal }, { $literal_10 } o { $literal_11 }){ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_14 }Gestisci l'accesso a { $term } e i token{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.thinking.run.chatgpt.sign.in.account.mismatch.run.thinking.chatgpt = account non corrispondente: esegui '{ $literal_1 }'

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.failed.outcome = non è stato possibile accedere a { $literal_1 }: { $outcome }

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.required.run = accesso a { $literal_2 } obbligatorio: esegui '{ $literal_1 }'

journal.cli.host.thinking.run.chatgpt.sign.in.signed.in.chatgpt.successfully = accesso a { $literal_1 } riuscito.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt = uscita da { $literal_1 }.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt.complete.sign = uscita da { $literal_1 }.{ $line_break }Per completare l'uscita, disconnetti solstone nelle impostazioni del tuo account { $literal_1 }.

journal.cli.host.thinking.run.chatgpt.status.plan.usage.declined = utilizzo del piano: rifiutato

journal.cli.host.thinking.run.chatgpt.status.status.not.signed.in = stato: senza accesso

journal.cli.host.thinking.run.chatgpt.status.status.signed.in = stato: con accesso

journal.cli.host.thinking.set.lane.help = uso: { $syntax }{ $line_break }{ $line_break }Imposta la corsia di ragionamento usata da { $literal }.{ $line_break }{ $line_break }argomenti posizionali:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_24 }Corsia di ragionamento da attivare.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_3 }fornitore { $argument }: { $literal_12 }, { $literal_13 }, { $literal_14 }, { $literal_15 } o { $literal_16 }{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }Nome del modello cloud { $argument }{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_8 }Radice del diario{ $line_break }

journal.cli.host.thinking.set.lane.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.usage = uso: { $syntax }{ $line_break }

journal.cli.host.top.failure = { $command_solstone_journal } top: { $error }

journal.cli.host.top.help = uso: { $syntax }{ $line_break }{ $line_break }Mostra in modo interattivo il servizio, l'osservazione, il compito e l'attività del brain.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }mostra questo messaggio di aiuto ed esci{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }attiva l'output dettagliato{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }attiva l'output di debug{ $line_break }

journal.cli.host.top.usage = uso: { $syntax }{ $line_break }

journal.cli.host.up.help = uso: { $syntax }{ $line_break }{ $line_break }Avvia il servizio del diario installato se non è in esecuzione, poi attendi finché non è pronto.{ $line_break }{ $line_break }opzioni:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }mostra questo messaggio di aiuto ed esci{ $line_break }

journal.cli.host.usage.commands = Uso:{ $line_break }{ $commands }

journal.cli.host.warm.development = Un GAP è un elemento fratello della disposizione di sviluppo che non è stato compilato e quindi non è stato esercitato; viene riportato per nome ma non fa fallire warm.

journal.cli.host.warm.inventory = Warm controlla i { $count } binari nativi dichiarati dalle foglie di pacchettizzazione maturin { $bindings } dell'host.

journal.cli.host.warm.linux.dynamic = Su Linux, { $solstone_core_speakers_analyze } e { $solstone_core_vad_analyze } caricano dinamicamente { $onnx } incluso nel pacchetto, e { $solstone_core_vulkan_probe } carica dinamicamente il loader { $vulkan } dell'host. Warm dimostra la risoluzione del loro loader e anche il loro avvio. { $solstone_core_describe } qui non è affermato né statico né dinamico; la validazione della wheel vieta solo { $ffmpeg } collegato dinamicamente.

journal.cli.host.warm.linux.static = Su Linux, { $solstone_core }, { $solstone_core_journal }, { $solstone_retention }, { $solstone_core_sol } e { $solstone_core_depict } sono collegati staticamente a musl. Warm dimostra che si avviano e raggiungono il proprio codice; non c'è una risoluzione del loader dinamico da dimostrare.

journal.cli.host.warm.mac = Su macOS ogni Mach-O collega { $library }. La proprietà non è vuota ma è diversa da Linux: lì warm mostra i fallimenti di firma, di quarantena e di { $dyld }.

journal.cli.host.warm.platforms = Righe di warm limitate alla piattaforma: { $rows }. Riportano NOT APPLICABLE quando la loro applicabilità non include questo host e non fanno fallire warm.

journal.cli.host.warm.repair.development = { $space_2 }riparazione: Compila questo crate in un ambiente di sviluppo configurato.

journal.cli.host.warm.repair.execution = { $space_2 }riparazione: Ripara il fallimento di esecuzione dell'host segnalato, poi reinstalla se serve lo strumento dell'host di appartenenza.

journal.cli.host.warm.repair.missing = { $space_2 }riparazione: Reinstalla o aggiorna lo strumento dell'host di appartenenza con la famiglia di programmi di installazione che ha creato questo ambiente.

journal.cli.host.warm.repair.platform = { $space_2 }riparazione: Nessuna riparazione necessaria su questa piattaforma.

journal.cli.host.warm.row.development = GAP { $binary }: { $path } è assente accanto all'eseguibile di sviluppo

journal.cli.host.warm.row.executable = CANNOT LOAD { $binary }: non è possibile risolvere l'eseguibile attuale: { $error }

journal.cli.host.warm.row.exit = RAN { $binary }: ha restituito l'uscita { $code }

journal.cli.host.warm.row.library = CANNOT LOAD { $binary }: non è stato possibile caricare la libreria condivisa { $library }

journal.cli.host.warm.row.missing = MISSING { $binary }: { $path } è assente accanto all'eseguibile in esecuzione

journal.cli.host.warm.row.no.code = CANNOT LOAD { $binary }: terminato senza un codice di uscita

journal.cli.host.warm.row.platform = NOT APPLICABLE { $binary }: il binario solo per Linux non è applicabile su questa piattaforma

journal.cli.host.warm.row.signal = CANNOT LOAD { $binary }: terminato dal segnale { $signal }

journal.cli.host.warm.row.spawn = CANNOT LOAD { $binary }: non è stato possibile generare { $path }: { $error }

journal.cli.host.warm.summary = { $quantity ->
    [one] Riepilogo: { $ran } eseguiti, { $missing } mancanti, { $cannot_load } non caricabili, { $gaps } lacuna, { $not_applicable } non applicabili.
    [many] Riepilogo: { $ran } eseguiti, { $missing } mancanti, { $cannot_load } non caricabili, { $gaps } di lacune, { $not_applicable } non applicabili.
   *[other] Riepilogo: { $ran } eseguiti, { $missing } mancanti, { $cannot_load } non caricabili, { $gaps } lacune, { $not_applicable } non applicabili.
}

journal.cli.host.warm.unresolved = { $space_2 }libreria non risolta: { $library }

journal.cli.host.windows.service.adopted.installation.has.no.journal.path = l'installazione adottata non ha un percorso del diario

journal.cli.host.windows.service.background.service.is.not.installed = il servizio in secondo piano non è installato

journal.cli.host.windows.service.background.service.stopped = servizio in secondo piano fermato

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.changed.run = non è stato possibile cambiare il supporto in secondo piano del tuo diario.{ $break_line }esegui `{ $command_solstone_journal_service_status }` per controllarlo.{ $break_line }dettagli: { $reason }

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.checked.try = non è stato possibile controllare il supporto in secondo piano del tuo diario.{ $break_line }riprova tra un momento. se continua a non riuscire, includi i dettagli sotto in una richiesta di supporto.{ $break_line }dettagli: { $reason }

journal.cli.host.windows.service.before.uninstall.deadline.is.out.of.range = la scadenza prima della disinstallazione è fuori intervallo

journal.cli.host.windows.service.cannot.identify.one.running.service.task = non è possibile identificare un solo compito di servizio in esecuzione

journal.cli.host.windows.service.cannot.resolve.task.artifact.directory = non è possibile risolvere la directory dell'artefatto del compito

journal.cli.host.windows.service.clean.uninstall.requires.an.adopted.installation.identity = la disinstallazione pulita richiede un'identità di installazione adottata

journal.cli.host.windows.service.could.not.clear.prior.update.intent = non è stato possibile cancellare l'intento di aggiornamento precedente: { $error }

journal.cli.host.windows.service.could.not.clear.windows.sign.in.resume = non è stato possibile cancellare la ripresa all'accesso di { $literal_windows }: { $error }

journal.cli.host.windows.service.could.not.consume.update.intent = non è stato possibile consumare l'intento di aggiornamento: { $error }

journal.cli.host.windows.service.could.not.inspect.current.executable = non è stato possibile ispezionare l'eseguibile attuale: { $error }

journal.cli.host.windows.service.could.not.load.installation.binding = non è stato possibile caricare il vincolo di installazione: { $error }

journal.cli.host.windows.service.could.not.locate.owner.base = non è stato possibile individuare la base del titolare: { $error }

journal.cli.host.windows.service.could.not.prepare.windows.sign.in.resume = non è stato possibile preparare la ripresa all'accesso di { $literal_windows }: { $error }

journal.cli.host.windows.service.could.not.read.update.intent.record = non è stato possibile leggere il registro dell'intento di aggiornamento: { $error }

journal.cli.host.windows.service.could.not.resolve.identity.root.from.executable.directory = non è stato possibile risolvere la radice dell'identità dalla directory dell'eseguibile

journal.cli.host.windows.service.could.not.resolve.root.token = non è stato possibile risolvere il token root: { $error }

journal.cli.host.windows.service.could.not.resolve.uninstall.root = non è stato possibile risolvere la radice di disinstallazione

journal.cli.host.windows.service.could.not.retrieve.user.sid = non è stato possibile recuperare il { $literal_sid } dell'account: { $error }

journal.cli.host.windows.service.could.not.write.update.intent.record = non è stato possibile scrivere il registro dell'intento di aggiornamento: { $error }

journal.cli.host.windows.service.direct.worker.stop.request.direct.worker.observation = richiesta di arresto del worker diretto: { $termination }; osservazione del worker diretto: { $observed }

journal.cli.host.windows.service.error.invalid.port = errore: porta non valida '{ $port }'

journal.cli.host.windows.service.installation.identity.census.is.unreadable = il censimento dell'identità di installazione è illeggibile

journal.cli.host.windows.service.installation.provider.location.is.unavailable = la posizione del fornitore di installazione non è disponibile

journal.cli.host.windows.service.installed.command.contains.invalid.text = il comando installato contiene testo non valido

journal.cli.host.windows.service.installed.task.action.did.not.match.its.readback = l'azione del compito installato non corrisponde alla sua rilettura

journal.cli.host.windows.service.is.registered.expected = { $registered } è registrato, atteso { $expected }

journal.cli.host.windows.service.it.belongs.to.a.different.installation.of.the.journal = appartiene a un'installazione diversa del diario

journal.cli.host.windows.service.it.is.registered.to.another.account = la registrazione è su un altro account

journal.cli.host.windows.service.it.runs.the.journal.at.expected = esegue il diario in { $registered }, atteso { $journal }

journal.cli.host.windows.service.journal = diario: { $error }

journal.cli.host.windows.service.journal.path.contains.invalid.text = il percorso del diario contiene testo non valido

journal.cli.host.windows.service.native.process.birth.changed = la nascita del processo nativo è cambiata

journal.cli.host.windows.service.native.process.birth.is.missing = manca la nascita del processo nativo

journal.cli.host.windows.service.native.process.pid.is.invalid = il PID del processo nativo non è valido

journal.cli.host.windows.service.native.windows.directory.is.unavailable = la directory nativa di { $literal_windows } non è disponibile

journal.cli.host.windows.service.no.reason = windows non ha dato alcuna ragione.

journal.cli.host.windows.service.scheduler.returned.no.task.run.instance = lo scheduler non ha restituito alcuna istanza di esecuzione del compito

journal.cli.host.windows.service.scheduler.rpc.may.have.changed.state.re.inspection.required = { $reason }; { $observation }; l'RPC dello scheduler può avere cambiato lo stato; è necessaria una nuova ispezione

journal.cli.host.windows.service.selected.journal.differs.from.the.saved.installation.binding = il diario selezionato è diverso dal vincolo di installazione salvato

journal.cli.host.windows.service.service.exited.abnormally.cleanup.remains.unverified = il servizio è uscito in modo anomalo; la pulizia resta non verificata

journal.cli.host.windows.service.service.forwarder.admission.is.invalid = l'ammissione del forwarder del servizio non è valida

journal.cli.host.windows.service.service.installation.guard.differs.from.the.saved.binding = la guardia di installazione del servizio è diversa dal vincolo salvato

journal.cli.host.windows.service.service.readiness.has.no.exact.native.identity = la prontezza del servizio non ha un'identità nativa esatta

journal.cli.host.windows.service.service.readiness.has.no.installed.forwarder.admission = la prontezza del servizio non ha un'ammissione del forwarder installato

journal.cli.host.windows.service.service.readiness.identity.changed = l'identità della prontezza del servizio è cambiata

journal.cli.host.windows.service.service.run.exited.during.admission = l'esecuzione del servizio è uscita durante l'ammissione

journal.cli.host.windows.service.service.scheduler.instance.does.not.match.its.admitted.forwarder = l'istanza dello scheduler del servizio non corrisponde al suo forwarder ammesso

journal.cli.host.windows.service.service.stop.deadline.elapsed.cleanup.remains.unverified = la scadenza di arresto del servizio è trascorsa; la pulizia resta non verificata

journal.cli.host.windows.service.service.task.changed.during.run.admission = il compito di servizio è cambiato durante l'ammissione dell'esecuzione

journal.cli.host.windows.service.service.task.changed.during.start = il compito di servizio è cambiato durante l'avvio

journal.cli.host.windows.service.service.task.changed.during.stop = il compito di servizio è cambiato durante l'arresto

journal.cli.host.windows.service.service.task.changed.while.recording.its.state = il compito di servizio è cambiato mentre veniva scritto il suo stato

journal.cli.host.windows.service.service.task.deletion.was.not.observed = l'eliminazione del compito di servizio non è stata osservata

journal.cli.host.windows.service.service.task.differs.from.the.current.installation.binding = il compito di servizio è diverso dal vincolo di installazione attuale

journal.cli.host.windows.service.service.task.instance.changed.during.stop = l'istanza del compito di servizio è cambiata durante l'arresto

journal.cli.host.windows.service.service.task.instance.changed.while.retaining.native.handles = l'istanza del compito di servizio è cambiata mentre tratteneva gli handle nativi

journal.cli.host.windows.service.service.task.is.absent.and.its.forwarder.cleanup.cannot.be = il compito di servizio è assente e non è possibile verificare la pulizia del suo forwarder

journal.cli.host.windows.service.service.task.is.not.installed = il compito di servizio non è installato

journal.cli.host.windows.service.stopped.background.support.for.your.journal.starts.again.when.you = Fermato: il supporto in secondo piano del tuo diario si avvia di nuovo al prossimo accesso.

journal.cli.host.windows.service.stopped.background.support.for.your.journal.stays.off.until.you = Fermato: il supporto in secondo piano del tuo diario resta disattivo finché non lo avvii o non apri l'app del diario.

journal.cli.host.windows.service.supervisor.readiness.not.ready = Prontezza del supervisore: non pronta

journal.cli.host.windows.service.supervisor.readiness.ready = Prontezza del supervisore: pronta

journal.cli.host.windows.service.systemroot.is.unavailable = SystemRoot non è disponibile

journal.cli.host.windows.service.task = Compito: { $task }

journal.cli.host.windows.service.task.control.deadline.elapsed.before.spawn = la scadenza di controllo del compito è trascorsa prima della generazione

journal.cli.host.windows.service.task.control.exchange.failed = scambio di controllo del compito non riuscito: { $error }

journal.cli.host.windows.service.task.control.exchange.failed.worker.channel.closed = scambio di controllo del compito non riuscito: canale del worker chiuso

journal.cli.host.windows.service.task.control.exchange.timed.out = lo scambio di controllo del compito è scaduto

journal.cli.host.windows.service.task.control.input.exceeds.its.byte.limit = l'input di controllo del compito supera il suo limite di byte

journal.cli.host.windows.service.task.control.output.exceeds.its.byte.limit = l'output di controllo del compito supera il suo limite di byte

journal.cli.host.windows.service.task.control.spawn.failed = generazione di controllo del compito non riuscita: { $error }

journal.cli.host.windows.service.task.control.stdio.missing = stdio di controllo del compito mancante

journal.cli.host.windows.service.task.control.worker.exited = il worker di controllo del compito è uscito

journal.cli.host.windows.service.task.operation.returned.inconsistent.evidence = l'operazione del compito ha restituito una prova incoerente

journal.cli.host.windows.service.task.operation.returned.invalid.json = l'operazione del compito ha restituito { $literal_json } non valido

journal.cli.host.windows.service.task.request.is.not.a.json.object = la richiesta del compito non è un oggetto { $literal_json }

journal.cli.host.windows.service.task.xml.readback.missing = rilettura XML del compito mancante

journal.cli.host.windows.service.the.installed.solstone.exe.facade.is.unavailable = la facciata solstone.exe installata non è disponibile

journal.cli.host.windows.service.the.registered.task.has.no.readable.definition = il compito registrato non ha una definizione leggibile

journal.cli.host.windows.service.update.intent.record.is.invalid = il registro dell'intento di aggiornamento non è valido

journal.cli.host.windows.service.windows.run.key.was.not.opened = la chiave Run di { $literal_windows } non è stata aperta

journal.cli.host.windows.service.windows.service.action.contains.non.unicode.text = l'azione del servizio windows contiene testo non-Unicode

journal.cli.host.windows.service.windows.service.action.differs.from.the.complete.saved.binding = l'azione del servizio windows è diversa dal vincolo salvato completo

journal.cli.host.windows.service.windows.service.binding.unavailable = vincolo del servizio windows non disponibile

journal.cli.host.windows.service.windows.sign.in.resume.command.exceeds.the.run.key.limit = il comando di ripresa all'accesso di { $literal_windows } supera il limite della chiave Run o ha un percorso non valido

journal.cli.host.windows.service.windows.sign.in.resume.failed.windows.error = ripresa all'accesso di { $literal_windows } { $action } non riuscita (errore di { $literal_windows } { $code })

journal.cli.host.windows.service.windows.sign.in.resume.registry.readback.differed = la rilettura del registro di sistema della ripresa all'accesso di { $literal_windows } è diversa

journal.cli.host.windows.service.your.journal.didn.t.report.ready.so.the.start.can = il tuo diario non ha segnalato di essere pronto, quindi l'avvio non può essere confermato. esegui `{ $command_solstone_journal_service } logs` per vedere perché

journal.cli.host.windows.service.your.journal.is.ready = il tuo diario è pronto

journal.cli.install.archive.binary = { $binary_name } assente dall'archivio compresso

journal.cli.install.archive.digest = { $algorithm } non corrispondente: atteso { $expected }, ottenuto { $actual }

journal.cli.install.archive.download = non è stato possibile scaricare: { $message }

journal.cli.install.archive.host = host di scaricamento rifiutato: { $host }

journal.cli.install.archive.origin = origine dello scaricamento non disponibile su { $host }: { $message }

journal.cli.install.archive.path = l'elemento dell'archivio compresso fuoriesce dalla destinazione: { $path }

journal.cli.install.archive.redirect = limite di salti di reindirizzamento dello scaricamento superato: { $limit }

journal.cli.install.archive.scheme = schema di scaricamento rifiutato per { $host }: { $scheme }

journal.cli.install.archive.size = { $quantity ->
    [one] dimensione dello scaricamento non corrispondente: atteso { $expected } byte, ottenuto { $actual }
    [many] dimensione dello scaricamento non corrispondente: atteso { $expected } di byte, ottenuto { $actual }
   *[other] dimensione dello scaricamento non corrispondente: atteso { $expected } byte, ottenuto { $actual }
}

journal.cli.install.archive.userinfo = l'authority dell'URL di scaricamento non deve includere userinfo: { $authority }

journal.cli.install.backend.metal = il backend deve essere "metal" quando è fornito

journal.cli.install.backend.unavailable = il backend locale richiesto non è disponibile su questa piattaforma

journal.cli.install.component.packaged = difetto di build: questo componente viene fornito nel pacchetto, e riprovare non servirà

journal.cli.install.component.unfetched = difetto di build: questa build non scarica questo componente, e riprovare non servirà

journal.cli.install.coreml.asset.digest = discrepanza dell'hash della risorsa parakeet { $coreml } per { $file }: atteso { $expected }, trovato { $found }

journal.cli.install.coreml.asset.incomplete = la risorsa del modello del parlato è incompleta

journal.cli.install.coreml.asset.missing = risorsa parakeet { $coreml } mancante: { $file }; installala con: { $install_command }

journal.cli.install.coreml.asset.read = non è stato possibile leggere { $file }: { $error }

journal.cli.install.coreml.asset.size = discrepanza di dimensione della risorsa parakeet { $coreml } per { $file }: atteso { $expected }, trovato { $found }

journal.cli.install.coreml.attempt.changed = il tentativo di installazione del parlato è cambiato.

journal.cli.install.coreml.cache.directory = la directory della cache della sentinella parakeet { $coreml } ({ $installed }) non corrisponde a quella configurata ({ $configured })

journal.cli.install.coreml.catalog.conflict = le righe del catalogo { $artifact } hanno versioni in conflitto

journal.cli.install.coreml.catalog.empty = nessuna riga del catalogo { $artifact } trovata

journal.cli.install.coreml.catalog.mismatch = discrepanza di versione del catalogo parakeet { $coreml }: trovato { $found }, atteso { $expected }

journal.cli.install.coreml.platform = parakeet { $coreml } è supportato solo su { $host }, non su { $os_name }/{ $arch }

journal.cli.install.coreml.publication.lock = non è stato possibile acquisire il blocco di pubblicazione: { $error }

journal.cli.install.coreml.publish.restore = { $error }; non è stato possibile ripristinare: { $restore }

journal.cli.install.coreml.sentinel.not.ready = la sentinella parakeet { $coreml } non è pronta: { $path }; installala con: { $install_command }

journal.cli.install.coreml.sentinel.removed = la sentinella vigente è stata rimossa prima della pubblicazione

journal.cli.install.cuda.integrity = errore di integrità dell'ambiente di esecuzione cuda

journal.cli.install.disk.insufficient = spazio insufficiente per finire di scaricare il modello

journal.cli.install.fingerprint.artifact = { $field } mancante

journal.cli.install.fingerprint.backend = backend mancante

journal.cli.install.fingerprint.missing = impronta del tentativo mancante

journal.cli.install.fingerprint.runtime = chiave dell'artefatto dell'ambiente di esecuzione mancante

journal.cli.install.fit.artifact.local = artefatti del fornitore locale

journal.cli.install.fit.artifact.parakeet = artefatti di parakeet.cpp

journal.cli.install.fit.artifact.rfdetr = artefatti di rf-detr.cpp

journal.cli.install.fit.check.disk = disco

journal.cli.install.fit.check.gpu = gpu

journal.cli.install.fit.check.platform = piattaforma

journal.cli.install.fit.check.ram = ram

journal.cli.install.fit.disk.insufficient = spazio su disco insufficiente per { $requirement } (servono { $required } GB, ci sono { $available } GB liberi)

journal.cli.install.fit.disk.insufficient.named = spazio su disco insufficiente per { $requirement } (servono { $required } GB, ci sono { $available } GB liberi): { $known }

journal.cli.install.fit.disk.insufficient.named.unknown = spazio su disco insufficiente per { $requirement } (servono { $required } GB, ci sono { $available } GB liberi): { $known }; dimensione di scaricamento sconosciuta per { $unknown }

journal.cli.install.fit.disk.insufficient.unknown = spazio su disco insufficiente per { $requirement } (servono { $required } GB, ci sono { $available } GB liberi); dimensione di scaricamento sconosciuta per { $unknown }

journal.cli.install.fit.disk.item.cuda.tarball = CUDA tarball di llama-server

journal.cli.install.fit.disk.item.gguf.model = Modello GGUF

journal.cli.install.fit.disk.item.llama.tarball = tarball di llama-server

journal.cli.install.fit.disk.item.mlx.model = Modello MLX

journal.cli.install.fit.disk.item.mmproj = mmproj

journal.cli.install.fit.disk.item.parakeet.cpu = tarball del server CPU di parakeet

journal.cli.install.fit.disk.item.parakeet.model = modello GGUF di parakeet

journal.cli.install.fit.disk.item.parakeet.vulkan = tarball del server Vulkan di parakeet

journal.cli.install.fit.disk.item.rfdetr.cli = binario CLI di rf-detr

journal.cli.install.fit.disk.item.rfdetr.model = modello GGUF di rf-detr

journal.cli.install.fit.disk.ok = { $available } GB liberi per { $required } GB { $requirement }

journal.cli.install.fit.disk.ok.unknown = { $available } GB liberi; per { $requirement } servono { $required } GB; dimensione di scaricamento sconosciuta per { $unknown }

journal.cli.install.fit.disk.overflow = overflow dello spazio su disco disponibile

journal.cli.install.fit.disk.requirement.bundled = installazione delle risorse incluse

journal.cli.install.fit.disk.requirement.downloads = scaricamenti noti

journal.cli.install.fit.disk.unavailable = non è stato possibile verificare lo spazio su disco disponibile in { $path }: { $error }

journal.cli.install.fit.disk.unavailable.unknown = non è stato possibile verificare lo spazio su disco disponibile in { $path }: { $error }; dimensione di scaricamento sconosciuta per { $unknown }

journal.cli.install.fit.disk.unsupported = l'ispezione dello spazio su disco non è supportata su questa piattaforma

journal.cli.install.fit.gpu.cuda.memory.unknown = il backend risolto è { $backend }: { $reason }; la memoria della GPU è sconosciuta

journal.cli.install.fit.gpu.cuda.ok = CUDA backend selezionato: { $reason }

journal.cli.install.fit.gpu.cuda.ok.cpu = CUDA backend selezionato: { $reason }; un modello viene eseguito sulla tua GPU; la trascrizione viene eseguita sulla tua CPU su questa macchina

journal.cli.install.fit.gpu.cuda.ok.unified = CUDA backend selezionato: { $reason }; la memoria di tiering della GPU usa MemAvailable di sistema

journal.cli.install.fit.gpu.cuda.ok.unified.cpu = CUDA backend selezionato: { $reason }; la memoria di tiering della GPU usa MemAvailable di sistema; un modello viene eseguito sulla tua GPU; la trascrizione viene eseguita sulla tua CPU su questa macchina

journal.cli.install.fit.gpu.cuda.probe.unavailable = Sonda della GPU NVIDIA non disponibile; il backend risolto è { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.below.bar = GPU { $name } ha { $available } GB; i modelli locali inclusi richiedono almeno 6 GB; il backend risolto è { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.below.bar.cpu = GPU { $name } ha { $available } GB; i modelli locali inclusi richiedono almeno 6 GB; il backend risolto è { $backend }: { $reason }; un modello viene eseguito sulla tua GPU; la trascrizione viene eseguita sulla tua CPU su questa macchina

journal.cli.install.fit.gpu.vulkan.eligible = GPU Vulkan selezionata: { $name }; il backend risolto è { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.eligible.cpu = GPU Vulkan selezionata: { $name }; il backend risolto è { $backend }: { $reason }; un modello viene eseguito sulla tua GPU; la trascrizione viene eseguita sulla tua CPU su questa macchina

journal.cli.install.fit.gpu.vulkan.memory.unknown = Non è stato possibile leggere la memoria della GPU; il backend risolto è { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.no.hardware = nessuna GPU hardware Vulkan selezionata; il backend risolto è { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.probe.incomplete = La sonda della GPU Vulkan non è stata completata; il backend risolto è { $backend }: { $reason }

journal.cli.install.fit.header = { $artifact } controllo di compatibilità: { $overall }

journal.cli.install.fit.platform.embedded.blocked = l'app del diario non fornisce questo ambiente di esecuzione locale

journal.cli.install.fit.platform.embedded.ok = l'ambiente di esecuzione locale è incluso nell'app del diario

journal.cli.install.fit.platform.llama.blocked = Nessun artefatto llama-server con pin per la piattaforma { $key }

journal.cli.install.fit.platform.llama.ok = l'artefatto llama-server con pin è disponibile per { $key }

journal.cli.install.fit.platform.mlx.blocked = MLX richiede macos su Apple silicon; ottenuto { $os }/{ $arch }

journal.cli.install.fit.platform.mlx.ok = L'ambiente di esecuzione MLX è incluso con il diario

journal.cli.install.fit.platform.parakeet.ok = gli artefatti parakeet.cpp con pin sono disponibili per { $key }

journal.cli.install.fit.platform.rfdetr.blocked = rf-detr.cpp richiede darwin/arm64, linux/x86_64 o linux/arm64; ottenuto { $os }/{ $arch }

journal.cli.install.fit.platform.rfdetr.ok = gli artefatti rf-detr.cpp con pin sono disponibili per { $key }

journal.cli.install.fit.platform.rfdetr.windows.ok = il payload firmato del pacchetto rf-detr.cpp è supportato per { $os }/{ $arch }

journal.cli.install.fit.platform.windows.llama.blocked = llama-server richiede un'architettura windows supportata; ottenuto { $os }/{ $arch }

journal.cli.install.fit.platform.windows.llama.ok = il payload firmato del pacchetto llama-server è supportato per windows/{ $arch }

journal.cli.install.fit.ram.insufficient = memoria RAM insufficiente per { $model } (servono { $required } GB disponibili, ci sono { $available } GB disponibili)

journal.cli.install.fit.ram.ok = la memoria disponibile di { $available } GB soddisfa il requisito di { $required } GB per { $model }

journal.cli.install.fit.ram.unknown = non è stato possibile verificare la memoria disponibile per { $model }

journal.cli.install.fit.row = [{ $severity }] { $name }: { $detail }

journal.cli.install.fit.severity.blocked = bloccato

journal.cli.install.fit.severity.ok = ok

journal.cli.install.fit.severity.unknown = sconosciuto

journal.cli.install.fit.severity.warning = avviso

journal.cli.install.hardware.apple.memory = il ragionamento locale richiede almeno { $minimum } di memoria

journal.cli.install.hardware.memory.minimum = { $available_bytes ->
    [one] { $vram } locale del dispositivo sotto il minimo di { $minimum }: { $available_bytes } byte
    [many] { $vram } locale del dispositivo sotto il minimo di { $minimum }: { $available_bytes } di byte
   *[other] { $vram } locale del dispositivo sotto il minimo di { $minimum }: { $available_bytes } byte
}

journal.cli.install.hardware.memory.unknown = non è stato possibile segnalare la memoria dalla sonda { $vulkan }

journal.cli.install.hardware.no.hardware = nessuna GPU hardware { $vulkan } trovata

journal.cli.install.hardware.probe.required = osservazione della GPU { $vulkan } richiesta su Windows

journal.cli.install.lease.held = il lease di installazione di { $provider } è trattenuto

journal.cli.install.lease.regular = il lease di installazione non è un file regolare

journal.cli.install.member.cuda.arch = architettura cuda { $arch } non supportata

journal.cli.install.member.cuda.arch.key = chiave di architettura cuda { $artifact_key } non supportata

journal.cli.install.member.cuda.missing = membro cuda obbligatorio mancante: { $name }

journal.cli.install.member.cuda.regular = il membro cuda obbligatorio non è un file regolare: { $name }

journal.cli.install.member.link.broken = link rotto nella catena per { $name }

journal.cli.install.member.link.cycle = ciclo di link rilevato per { $name }

journal.cli.install.member.link.missing = membro link obbligatorio mancante: { $name }

journal.cli.install.member.link.read = non è stato possibile leggere il link { $name }: { $error }

journal.cli.install.member.link.regular = il link { $name } non si risolve in un file regolare

journal.cli.install.member.link.target = discrepanza della destinazione del link { $name }: atteso { $target }, ottenuto { $found }

journal.cli.install.member.link.type = il membro link obbligatorio non è un collegamento simbolico: { $name }

journal.cli.install.member.link.unreadable = non è stato possibile leggere il link nella catena per { $name }

journal.cli.install.member.regular.missing = membro regolare obbligatorio mancante: { $name }

journal.cli.install.member.regular.type = il membro regolare obbligatorio non è un file regolare: { $name }

journal.cli.install.metal.platform = l'inferenza locale { $metal } non è supportata su { $key }

journal.cli.install.mlx.digest.read = non è stato possibile eseguire { $function } per { $file }: { $error }

journal.cli.install.mlx.directory.explicit = la directory esplicita del modello { $mlx } non esiste: { $path }

journal.cli.install.mlx.directory.invalid = la directory del modello non è una directory: { $path }

journal.cli.install.mlx.directory.override = { $env } non esiste: { $path }

journal.cli.install.mlx.directory.read = non è stato possibile leggere la directory del modello: { $error }

journal.cli.install.mlx.directory.unset = { $env } non è impostato o non è una directory

journal.cli.install.mlx.entry.read = non è stato possibile leggere l'elemento: { $error }

journal.cli.install.mlx.file.digest = discrepanza sha256 per { $file }: atteso { $expected } ottenuto { $actual }

journal.cli.install.mlx.file.missing = file obbligatorio mancante: { $file }

journal.cli.install.mlx.file.regular = non è un file regolare: { $file }

journal.cli.install.mlx.file.size = discrepanza di dimensione per { $file }: atteso { $expected } ottenuto { $actual }

journal.cli.install.mlx.helper.explicit = l'ausiliario esplicito { $mlx } non è valido o manca { $library }: { $path }

journal.cli.install.mlx.helper.invalid = il binario dell'ausiliario non è valido: { $path }

journal.cli.install.mlx.helper.missing = eseguibile { $helper } con il bundle di risorse { $library } non trovato

journal.cli.install.mlx.helper.override = { $env } non è valido o manca { $library }: { $path }

journal.cli.install.mlx.input.extra = input aggiuntivo non verificato del modello: { $file }

journal.cli.install.mlx.metadata.read = non è stato possibile leggere i metadati di { $file }: { $error }

journal.cli.install.mlx.override.failed = non è stato possibile verificare l'override { $mlx }

journal.cli.install.mlx.pins.empty = elenco dei pin vuoto

journal.cli.install.mlx.pins.missing = nessun pin fornito

journal.cli.install.mlx.platform = { $mlx } richiede { $os }

journal.cli.install.mlx.type.read = tipo di file non riuscito: { $error }

journal.cli.install.pdfium.windows.executable.location = l'eseguibile del diario in esecuzione non è nella directory bin del pacchetto: { $path }

journal.cli.install.pdfium.windows.executable.parent = l'eseguibile del diario in esecuzione non ha una directory superiore: { $path }

journal.cli.install.pdfium.windows.executable.read = non è stato possibile determinare l'eseguibile del diario in esecuzione: { $error }

journal.cli.install.pdfium.windows.package.root = la directory bin del pacchetto non ha una radice del pacchetto: { $path }

journal.cli.install.pdfium.windows.payload.refuses = il payload firmato dell'app PDF rifiuta { $member }: { $error }

journal.cli.install.pdfium.windows.payload.verify = non è stato possibile verificare il payload firmato dell'app PDF: { $error }

journal.cli.install.pdfium.windows.runtime = La verifica del pacchetto PDF di Windows richiede un ambiente di esecuzione Windows

journal.cli.install.pin.missing = nessun pin per { $key }

journal.cli.install.registry.artifact.ambiguous = il catalogo ha più artefatti { $unit } corrispondenti

journal.cli.install.registry.artifact.missing = il catalogo non ha un artefatto { $unit } corrispondente

journal.cli.install.registry.platform = nessuna mappatura di piattaforma nel catalogo per { $key }

journal.cli.install.rfdetr.asset.missing = risorsa rf-detr mancante: { $label }

journal.cli.install.rfdetr.binary.missing = L'archivio compresso estratto non conteneva { $binary }

journal.cli.install.rfdetr.bundled.missing = risorsa rf-detr inclusa mancante: { $filename }; reinstalla il pacchetto del diario

journal.cli.install.rfdetr.contract.missing = contratto di consegna compilato di rf-detr mancante

journal.cli.install.rfdetr.contract.slot = lo slot { $actual } del contratto di consegna compilato di rf-detr non corrisponde a { $expected }

journal.cli.install.rfdetr.member.mismatch = il membro binario rf-detr { $actual } non corrisponde a { $expected }

journal.cli.install.rfdetr.member.outside = il binario rf-detr era fuori dalla radice di estrazione: { $error }

journal.cli.install.rfdetr.probe.failed = sonda del modello rf-detr non riuscita: { $result }

journal.cli.install.rfdetr.sha256.mismatch = discrepanza sha256 per { $label }

journal.cli.install.rfdetr.sidecar.invalid = sidecar rf-detr non valido: { $path }: { $error }

journal.cli.install.rfdetr.sidecar.mismatch = lo sidecar rf-detr non corrisponde agli artefatti con pin

journal.cli.install.rfdetr.sidecar.missing = sidecar rf-detr mancante: { $path }

journal.cli.install.rfdetr.size.mismatch = discrepanza di dimensione per { $label }: atteso { $expected }, ottenuto { $actual }

journal.cli.install.rfdetr.windows.detect.failed = rfdetr-cli detect non riuscito

journal.cli.install.rfdetr.windows.executable.location = l'eseguibile del diario in esecuzione non è nella directory bin del pacchetto: { $path }

journal.cli.install.rfdetr.windows.executable.parent = l'eseguibile del diario in esecuzione non ha una directory superiore

journal.cli.install.rfdetr.windows.executable.read = non è stato possibile determinare l'eseguibile del diario in esecuzione: { $error }

journal.cli.install.rfdetr.windows.package.root = la directory bin del pacchetto non ha una radice del pacchetto

journal.cli.install.rfdetr.windows.result.limit = Il risultato RF-DETR ha superato il suo limite di byte

journal.cli.install.rfdetr.windows.runtime = La verifica del pacchetto RF-DETR di Windows richiede un ambiente di esecuzione Windows

journal.cli.install.rfdetr.windows.verify.failed = Non è stato possibile verificare il pacchetto RF-DETR di Windows: { $error }

journal.cli.install.runtime.unavailable = l'app del diario non fornisce questo ambiente di esecuzione locale

journal.cli.install.speech.platform = { $program } non supportato su { $os_name }/{ $arch }

journal.cli.install.staging.binary.parent = il binario non ha una directory superiore

journal.cli.install.staging.bundle.occupied = la destinazione del bundle esiste già: { $path }

journal.cli.install.staging.target.name = la destinazione non ha un nome di file

journal.cli.install.staging.target.parent = la destinazione non ha una directory superiore

journal.cli.install.status.conflict.different.attempt.while.install.in.flight = conflitto dello stato di installazione: tentativo diverso durante un'installazione in corso

journal.cli.install.status.conflict.illegal.install.status.transition = conflitto dello stato di installazione: transizione illegale

journal.cli.install.status.conflict.install.attempt.id.changed = conflitto dello stato di installazione: l'id del tentativo di installazione è cambiato

journal.cli.install.status.conflict.install.attempt.is.no.longer.in.flight = conflitto dello stato di installazione: il tentativo di installazione non è più in corso

journal.cli.install.status.conflict.install.progress.can.only.be.bumped.for = conflitto dello stato di installazione: il progresso dell'installazione può essere incrementato solo per gli stati in corso

journal.cli.install.status.conflict.install.target.fingerprint.changed = conflitto dello stato di installazione: l'impronta della destinazione di installazione è cambiata

journal.cli.install.status.conflict.interrupted.attempt.id.does.not.match = conflitto dello stato di installazione: l'id del tentativo interrotto non corrisponde

journal.cli.install.status.conflict.interrupted.target.fingerprint.does.not.match = conflitto dello stato di installazione: l'impronta della destinazione interrotta non corrisponde

journal.cli.install.status.conflict.new.in.flight.attempt.reused.attempt.id = conflitto dello stato di installazione: un nuovo tentativo in corso ha riutilizzato l'id del tentativo

journal.cli.install.status.conflict.non.idle.install.status.requires.attempt.id = conflitto dello stato di installazione: uno stato di installazione non inattivo richiede l'id del tentativo

journal.cli.install.status.conflict.only.in.flight.installs.can.be.interrupted = conflitto dello stato di installazione: si possono interrompere solo le installazioni in corso

journal.cli.install.status.conflict.stale.install.status.revision = conflitto dello stato di installazione: revisione dello stato di installazione non aggiornata

journal.cli.install.status.interrupted = lo scaricamento è stato interrotto

journal.cli.install.status.lock = errore del blocco di stato: { $error }

journal.cli.install.status.malformed.file = stato di installazione malformato: { $path }

journal.cli.install.status.malformed.initial.install.attempt.state.must.be.in = stato di installazione malformato: lo stato iniziale del tentativo di installazione deve essere in corso

journal.cli.install.status.malformed.invalid.provider = stato di installazione malformato: stato non valido per { $provider }

journal.cli.install.status.malformed.owner.must.be.an.object.or.null = stato di installazione malformato: owner deve essere un oggetto o null

journal.cli.install.status.malformed.providers = stato di installazione malformato: lo stato di installazione del fornitore deve essere uno di: [{ $listed }]

journal.cli.install.status.malformed.state = stato di installazione malformato: stato di installazione sconosciuto: { $state }

journal.cli.install.terminal.write = l'installazione nativa non è riuscita e non è stato possibile scrivere lo stato

journal.cli.install.windows.digest.absent = il manifest del payload firmato non dichiara l'hash per { $path }

journal.cli.install.windows.executable.location = l'eseguibile del diario in esecuzione non è nella directory bin del pacchetto: { $path }

journal.cli.install.windows.executable.parent = l'eseguibile del diario in esecuzione non ha una directory superiore

journal.cli.install.windows.executable.read = non è stato possibile determinare l'eseguibile del diario in esecuzione: { $error }

journal.cli.install.windows.package.root = la directory bin del pacchetto non ha una radice del pacchetto

journal.cli.install.windows.package.runtime = La verifica del pacchetto di ragionamento locale di Windows richiede un ambiente di esecuzione Windows

journal.cli.install.windows.payload.bytes = byte{ $line_break }  { $detail }

journal.cli.install.windows.payload.case.collision = collisione di maiuscole e minuscole{ $line_break }  { $detail }

journal.cli.install.windows.payload.digest = hash{ $line_break }  { $detail }

journal.cli.install.windows.payload.empty = vuoto{ $line_break }  { $detail }

journal.cli.install.windows.payload.lock.digest = cargo-lock-sha256{ $line_break }  { $detail }

journal.cli.install.windows.payload.manifest = manifest{ $line_break }  { $detail }

journal.cli.install.windows.payload.missing = mancante{ $line_break }  { $detail }

journal.cli.install.windows.payload.missing.member = membro mancante{ $line_break }  { $detail }

journal.cli.install.windows.payload.not.regular = non regolare{ $line_break }  { $detail }

journal.cli.install.windows.payload.reparse.or.symlink = reparse o collegamento simbolico{ $line_break }  { $detail }

journal.cli.install.windows.payload.schema = schema{ $line_break }  { $detail }

journal.cli.install.windows.payload.signature = firma{ $line_break }  { $detail }

journal.cli.install.windows.payload.source.commit = commit di origine{ $line_break }  { $detail }

journal.cli.install.windows.payload.target = destinazione{ $line_break }  { $detail }

journal.cli.install.windows.payload.unexpected.member = membro inatteso{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsafe.path = percorso non sicuro{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsorted.or.duplicate = non ordinato o duplicato{ $line_break }  { $detail }

journal.cli.install.windows.platform = piattaforma Windows non supportata: { $key }

journal.cli.install.windows.target.different = la destinazione del payload firmato differisce da { $key }

journal.cli.install.windows.target.unsupported = la destinazione del payload firmato non è supportata

journal.cli.path.create = non è stato possibile creare la directory del diario ({ $source }): { $path }: { $error }

journal.cli.path.home = journal-path non riuscito: non è stato possibile determinare la directory personale

journal.cli.path.utf8 = journal-path non riuscito: la configurazione non è un { $encoding } valido
