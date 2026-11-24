journal.window.date.months.short.apr = abr

journal.window.date.months.short.aug = ago

journal.window.date.months.short.dec = dic

journal.window.date.months.short.feb = feb

journal.window.date.months.short.jan = ene

journal.window.date.months.short.jul = jul

journal.window.date.months.short.jun = jun

journal.window.date.months.short.mar = mar

journal.window.date.months.short.may = may

journal.window.date.months.short.nov = nov

journal.window.date.months.short.oct = oct

journal.window.date.months.short.sep = sep

journal.window.date.today = hoy

journal.window.date.tomorrow = mañana

journal.window.date.weekdays.friday = viernes

journal.window.date.weekdays.monday = lunes

journal.window.date.weekdays.saturday = sábado

journal.window.date.weekdays.short.fri = vie

journal.window.date.weekdays.short.mon = lun

journal.window.date.weekdays.short.sat = sáb

journal.window.date.weekdays.short.sun = dom

journal.window.date.weekdays.short.thu = jue

journal.window.date.weekdays.short.tue = mar

journal.window.date.weekdays.short.wed = mié

journal.window.date.weekdays.sunday = domingo

journal.window.date.weekdays.thursday = jueves

journal.window.date.weekdays.tuesday = martes

journal.window.date.weekdays.wednesday = miércoles

journal.window.date.yesterday = ayer

journal.window.health.accessible.label.active = activo

journal.window.health.accessible.label.none = ninguno

journal.window.health.accessible.label.services.all.running = servicios: todos en ejecución

journal.window.health.accessible.label.services.need.attention = servicios: necesitan atención

journal.window.health.accessible.label.services.status.unavailable = servicios: estado no disponible

journal.window.health.accessible.label.services.warning = servicios: advertencia

journal.window.health.accessible.label.stale = desactualizado

journal.window.health.accessible.label.stale.not.responding = desactualizado, no responde

journal.window.health.accessible.name.health.loading = estado: cargando

journal.window.health.accessible.name.health.summary.metrics = métricas del resumen de estado

journal.window.health.accessible.name.queues.loading = colas: cargando

journal.window.health.accessible.name.schedules.loading = horarios: cargando

journal.window.health.accessible.name.services.loading = servicios: cargando

journal.window.health.accessible.name.set.aside.work.pages = páginas de trabajo apartado

journal.window.health.accessible.name.tasks.loading = tareas: cargando

journal.window.health.accessible.name.thinking.loading = razonamiento: cargando

journal.window.health.action.a.required.field.is.missing = falta un campo obligatorio.

journal.window.health.action.import.retry.from.stage.stage.will.be.available.in = El reintento de importación desde la etapa{ $slot1 }{ $stage }{ $slot2 }estará disponible en una actualización futura

journal.window.health.action.import.retry.will.be.available.in.a.future.update = El reintento de importación estará disponible en una actualización futura

journal.window.health.action.missing.day = Falta { $slot1 }

journal.window.health.action.missing.import.id = Falta { $slot1 }

journal.window.health.action.missing.unit = Falta { $slot1 }

journal.window.health.action.no.model.is.chosen.yet.choose.one.in.thinking = todavía no hay un modelo elegido. elige uno en razonamiento y vuelve a intentarlo.

journal.window.health.action.one.of.those.values.couldn.t.be.used = no se pudo usar uno de esos valores.

journal.window.health.action.reprocess.failed.cause = no se pudo reprocesar:{ $slot1 }{ $cause }

journal.window.health.action.retry.identity.invalid = identidad de reintento no válida

journal.window.health.action.retry.record.unrecognized = registro de reintento no reconocido

journal.window.health.action.saved.request.could.not.be.read = no se pudo leer la solicitud guardada

journal.window.health.action.that.day.couldn.t.be.used = no se pudo usar ese día.

journal.window.health.action.that.work.has.changed.since.this.list.was.checked = ese trabajo cambió desde que se revisó esta lista. actualizando su estado.

journal.window.health.action.that.work.is.no.longer.set.aside = ese trabajo ya no está apartado

journal.window.health.action.the.background.service.could.not.be.reached.the.safety = no se pudo conectar con el servicio en segundo plano. el bloqueo de seguridad sigue en su lugar.

journal.window.health.action.the.retry.could.not.be.checked = no se pudo revisar el reintento.

journal.window.health.action.the.safety.hold.is.still.in.place.checking.the = el bloqueo de seguridad sigue en su lugar. revisando de nuevo el estado del diario.

journal.window.health.action.the.source.still.cannot.be.read.as.usable.media = la fuente todavía no se puede leer como contenido multimedia utilizable. reemplázala antes de reintentar

journal.window.health.action.this.work.is.still.held.after.checking.its.source = este trabajo sigue retenido después de revisar su fuente

journal.window.health.action.unknown.reprocess.flavor = Variante de reprocesamiento desconocida

journal.window.health.action.you.can.only.reprocess.past.days.today.and.future = solo puedes reprocesar días pasados — hoy y los días futuros todavía no están listos.

journal.window.health.action.your.request.is.saved.it.will.run.when.your = tu solicitud está guardada. se va a ejecutar cuando empiece el servicio en segundo plano de tu diario.

journal.window.health.action.your.retry.request.is.saved.the.journal.will.check = tu solicitud de reintento está guardada. el diario va a revisar este trabajo antes de reintentarlo.

journal.window.health.agent.errors.many = errores hoy

journal.window.health.agent.errors.one = error hoy

journal.window.health.backlog.action.process.now = priorizar el día

journal.window.health.backlog.action.redo.scratch = rehacer desde cero

journal.window.health.backlog.bucket.description = algunos días se reintentan automáticamente; otros necesitan tu ayuda. cada día muestra su estado actual.

journal.window.health.backlog.bucket.heading = días que necesitan ayuda

journal.window.health.backlog.catching.up.day = { $day } todavía se está poniendo al día →

journal.window.health.backlog.confirm.redo.scratch = ¿rehacer este día entero desde cero? esto vuelve a hacer las partes que ya terminaron, así que va a tardar más.

journal.window.health.backlog.day.badge = atascado

journal.window.health.backlog.media.segments =
    { $quantity ->
        [one] { $count } segmento en espera del procesamiento de contenido multimedia
        [many] { $count } segmentos en espera del procesamiento de contenido multimedia
       *[other] { $count } segmentos en espera del procesamiento de contenido multimedia
    }

journal.window.health.backlog.queued.feedback = solicitud guardada; el trabajo de hoy va primero

journal.window.health.backlog.scope = días recientes

journal.window.health.backlog.verdict.scoped = { $scope }: { $verdict }

journal.window.health.backlog.verdict.with.link = { $verdict } { $oldest_link_start }el más antiguo: { $day } →{ $oldest_link_end }

journal.window.health.brain.brain.record.unavailable = registro de brain no disponible

journal.window.health.brain.check.again = revisar de nuevo

journal.window.health.brain.thinking.status.unavailable = estado del razonamiento no disponible

journal.window.health.brain.unknown = desconocido

journal.window.health.button.label.allow.older.work = permitir trabajo anterior

journal.window.health.button.label.back.to.dashboard = ← volver al panel

journal.window.health.button.label.check.again = revisar de nuevo

journal.window.health.button.label.clear = limpiar

journal.window.health.button.label.export = exportar

journal.window.health.button.label.follow = seguir

journal.window.health.button.label.next = siguiente

journal.window.health.button.label.previous = anterior

journal.window.health.button.label.try.again = reintentar

journal.window.health.capture.audio.sounds =
    { $quantity ->
        [one] 1 sonido detectado
        [many] { $count } sonidos detectados
       *[other] { $count } sonidos detectados
    }

journal.window.health.capture.audio.sounds.saving =
    { $quantity ->
        [one] 1 sonido detectado · guardando
        [many] { $count } sonidos detectados · guardando
       *[other] { $count } sonidos detectados · guardando
    }

journal.window.health.capture.screen.displays =
    { $quantity ->
        [one] recibiendo { $count } pantalla, ~{ $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
        [many] recibiendo { $count } pantallas, ~{ $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
       *[other] recibiendo { $count } pantallas, ~{ $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
    }

journal.window.health.capture.terminal.snapshots =
    { $quantity ->
        [one] agregando { $count } instantánea, ~{ $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
        [many] agregando { $count } instantáneas, ~{ $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
       *[other] agregando { $count } instantáneas, ~{ $minute_quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }
    }

journal.window.health.device.activity.last.added = último agregado { $age }

journal.window.health.device.activity.last.reported = último reportado { $age }

journal.window.health.device.activity.paired = vinculado { $age }

journal.window.health.device.capture.idle =
    { $seconds_quantity ->
        [one] inactivo: { $seconds } s
        [many] inactivo: { $seconds } s
       *[other] inactivo: { $seconds } s
    }

journal.window.health.device.capture.offline.adding = { $device } está agregando a tu diario. el detalle en vivo no se está reportando ahora.

journal.window.health.device.capture.offline.last.added = { $device } agregó a tu diario { $age }. el detalle en vivo no se está reportando ahora.

journal.window.health.device.delivery.last.known = último conocido: { $status }

journal.window.health.device.delivery.unavailable = no se pudo revisar el estado de la entrega.

journal.window.health.device.delivery.unavailable.checked = no se pudo revisar el estado de la entrega. última revisión exitosa: { $checked_at }.

journal.window.health.device.delivery.unavailable.checked.rows = no se pudo revisar el estado de la entrega. última revisión exitosa: { $checked_at }. las filas de abajo muestran esa última revisión.

journal.window.health.device.delivery.unavailable.rows = no se pudo revisar el estado de la entrega. las filas de abajo muestran esa última revisión.

journal.window.health.device.processing.queued =
    { $queue_quantity ->
        [one] en cola: { $queued }
        [many] en cola: { $queued }
       *[other] en cola: { $queued }
    }

journal.window.health.device.processing.running.and.queued =
    { $running_quantity ->
        [one] en ejecución ({ $running }, +{ $queue_quantity ->
        [one] { $queued } en cola
        [many] { $queued } en cola
       *[other] { $queued } en cola
    })
        [many] en ejecución ({ $running }, +{ $queue_quantity ->
        [one] { $queued } en cola
        [many] { $queued } en cola
       *[other] { $queued } en cola
    })
       *[other] en ejecución ({ $running }, +{ $queue_quantity ->
        [one] { $queued } en cola
        [many] { $queued } en cola
       *[other] { $queued } en cola
    })
    }

journal.window.health.device.processing.running.count =
    { $running_quantity ->
        [one] en ejecución ({ $running })
        [many] en ejecución ({ $running })
       *[other] en ejecución ({ $running })
    }

journal.window.health.device.processing.running.with.queue =
    { $queue_quantity ->
        [one] en ejecución (+{ $queued } en cola)
        [many] en ejecución (+{ $queued } en cola)
       *[other] en ejecución (+{ $queued } en cola)
    }

journal.window.health.device.source.technical.details = detalles técnicos: { $source }

journal.window.health.device.upload.count =
    { $quantity ->
        [one] 1 subida rechazada
        [many] { $count } subidas rechazadas
       *[other] { $count } subidas rechazadas
    }

journal.window.health.device.upload.count.since =
    { $quantity ->
        [one] 1 subida rechazada desde { $day }
        [many] { $count } subidas rechazadas desde { $day }
       *[other] { $count } subidas rechazadas desde { $day }
    }

journal.window.health.device.upload.count.source =
    { $quantity ->
        [one] { $source }: 1 subida rechazada
        [many] { $source }: { $count } subidas rechazadas
       *[other] { $source }: { $count } subidas rechazadas
    }

journal.window.health.device.upload.count.source.since =
    { $quantity ->
        [one] { $source }: 1 subida rechazada desde { $day }
        [many] { $source }: { $count } subidas rechazadas desde { $day }
       *[other] { $source }: { $count } subidas rechazadas desde { $day }
    }

journal.window.health.device.upload.reason.code = código de motivo: { $code }

journal.window.health.device.upload.unknown = se rechazó una subida

journal.window.health.device.upload.unknown.since = se rechazó una subida desde { $day }

journal.window.health.device.upload.unknown.source = { $source }: se rechazó una subida

journal.window.health.device.upload.unknown.source.since = { $source }: se rechazó una subida desde { $day }

journal.window.health.disclosure.label.inspection.details = detalles de la inspección

journal.window.health.disclosure.label.review.items.on.hold = elementos en espera de revisión

journal.window.health.errors.advice.import = no se pudo importar. revisa el archivo de origen y reintenta, o { $log_link_start }ver los registros del importador de archivos{ $log_link_end }.

journal.window.health.errors.advice.import.stage = no se pudo importar en { $stage }. revisa el archivo de origen y reintenta, o { $log_link_start }ver los registros del importador de archivos{ $log_link_end }.

journal.window.health.errors.advice.service = se detectó un error del servicio. { $log_link_start }ver los registros del administrador del sistema{ $log_link_end } para ver detalles.

journal.window.health.errors.advice.thinking = los errores de razonamiento suelen desaparecer en la siguiente ejecución. { $log_link_start }ver los registros del entorno de ejecución del modelo{ $log_link_end } para ver detalles.

journal.window.health.errors.filter.day = mostrando los errores de { $day }

journal.window.health.errors.filter.day.talent = mostrando los errores de { $day } para { $talent }

journal.window.health.errors.filter.today = mostrando los errores de hoy

journal.window.health.errors.filter.today.talent = mostrando los errores de hoy para { $talent }

journal.window.health.glance.catching.up =
    { $quantity ->
        [one] poniéndose al día con { $count } tarea en segundo plano. última actualización { $age }.
        [many] poniéndose al día con { $count } tareas en segundo plano. última actualización { $age }.
       *[other] poniéndose al día con { $count } tareas en segundo plano. última actualización { $age }.
    }

journal.window.health.glance.checkin.stale = una ejecución del diario en { $run } dejó de reportarse a tu diario.

journal.window.health.glance.checkins.stale =
    { $quantity ->
        [one] una ejecución del diario en { $runs } dejó de reportarse a tu diario.
        [many] { $count } ejecuciones del diario dejaron de reportarse a tu diario: { $runs }.
       *[other] { $count } ejecuciones del diario dejaron de reportarse a tu diario: { $runs }.
    }

journal.window.health.glance.device.failing = tu diario no pudo aceptar subidas de { $device }.

journal.window.health.glance.device.silent = { $device } no agregó a tu diario en { $age }.

journal.window.health.glance.device.silent.no.age = { $device } no agregó a tu diario recientemente.

journal.window.health.glance.devices.count.many =
    { $quantity ->
        [one] 1 dispositivo está agregando a tu diario.
        [many] { $count } dispositivos están agregando a tu diario.
       *[other] { $count } dispositivos están agregando a tu diario.
    }

journal.window.health.glance.devices.empty.no.devices.are.paired.yet = todavía no hay dispositivos vinculados.

journal.window.health.glance.devices.failing =
    { $quantity ->
        [one] tu diario no pudo aceptar subidas de { $count } dispositivo: { $devices }.
        [many] tu diario no pudo aceptar subidas de { $count } dispositivos: { $devices }.
       *[other] tu diario no pudo aceptar subidas de { $count } dispositivos: { $devices }.
    }

journal.window.health.glance.devices.manage.manage.in.network = administrar en la red →

journal.window.health.glance.devices.pair.pair.a.device = vincular un dispositivo →

journal.window.health.glance.devices.see.see.your.devices.in.network = ver tus dispositivos en la red →

journal.window.health.glance.devices.silent =
    { $quantity ->
        [one] { $count } dispositivo no agregó a tu diario recientemente: { $devices }.
        [many] { $count } dispositivos no agregaron a tu diario recientemente: { $devices }.
       *[other] { $count } dispositivos no agregaron a tu diario recientemente: { $devices }.
    }

journal.window.health.glance.glance.checking.checking.where.your.journal.stands = revisando en qué va tu diario…

journal.window.health.glance.glance.client.one.of.your.devices.hasn.t = uno de tus dispositivos no conectó con tu diario recientemente.

journal.window.health.glance.glance.devices.delivery.from.some.of.your.devices = ahora no se conoce la entrega desde algunos de tus dispositivos.

journal.window.health.glance.glance.devices.your.devices.delivery.status.is.unavailable = el estado de entrega de tus dispositivos no está disponible ahora.

journal.window.health.glance.glance.ok.the.journal.s.live.services.and = los servicios en vivo del diario y el procesamiento están funcionando.

journal.window.health.glance.glance.services.the.journal.s.services.couldn.t = no se pudo conectar con los servicios del diario. revisa que tu diario esté en ejecución.

journal.window.health.glance.glance.work.queued.work.status.is.unavailable.right = el estado del trabajo en cola no está disponible ahora mismo.

journal.window.health.glance.glance.work.some.queued.work.is.on.hold = parte del trabajo en cola está en espera de una revisión de seguridad.

journal.window.health.glance.observe.local.your.journal.s.computer.isn.t = la computadora de tu diario todavía no se reporta.

journal.window.health.glance.observe.unknown.can.t.identify.your.journal.s = todavía no se puede identificar la computadora de tu diario.

journal.window.health.glance.service.attention = 1 servicio necesita atención: { $services }.

journal.window.health.glance.services.attention =
    { $quantity ->
        [one] { $count } servicio necesita atención: { $services }.
        [many] { $count } servicios necesitan atención: { $services }.
       *[other] { $count } servicios necesitan atención: { $services }.
    }

journal.window.health.heading.checking.device.activity = revisando la actividad del dispositivo…

journal.window.health.heading.device.activity.is.unavailable.right.now = la actividad del dispositivo no está disponible ahora mismo.

journal.window.health.heading.health.state.couldn.t.be.loaded = no se pudo cargar el estado

journal.window.health.heading.no.device.is.reporting.live.activity = ningún dispositivo está informando actividad en vivo ahora mismo.

journal.window.health.heading.no.devices.are.linked.to.your = todavía no hay dispositivos enlazados a tu diario.

journal.window.health.history.caught.up.since = el historial se puso al día hasta { $day }

journal.window.health.history.cost.consent = terminar { $recordings } grabaciones anteriores, { $day_outputs } resultados diarios y { $activities } actividades usa el modelo que elegiste y puede añadir cargos de tu proveedor. el trabajo de hoy va primero.

journal.window.health.history.counts =
    { $recording_quantity ->
        [one] { $recordings } grabación y { $output_quantity ->
        [one] { $outputs } resultado diario restante
        [many] { $outputs } resultados diarios restantes
       *[other] { $outputs } resultados diarios restantes
    }
        [many] { $recordings } grabaciones y { $output_quantity ->
        [one] { $outputs } resultado diario restante
        [many] { $outputs } resultados diarios restantes
       *[other] { $outputs } resultados diarios restantes
    }
       *[other] { $recordings } grabaciones y { $output_quantity ->
        [one] { $outputs } resultado diario restante
        [many] { $outputs } resultados diarios restantes
       *[other] { $outputs } resultados diarios restantes
    }
    }

journal.window.health.history.held.count =
    { $quantity ->
        [one] { $count } elemento apartado
        [many] { $count } elementos apartados
       *[other] { $count } elementos apartados
    }

journal.window.health.history.rate.hourly = aproximadamente { $rate } por hora

journal.window.health.history.summary.current = { $metrics }. hasta { $through_day }; hoy y ayer van por separado.

journal.window.health.history.summary.stale = última revisión: { $metrics }. hasta { $through_day }; hoy y ayer van por separado.

journal.window.health.history.unreadable.count =
    { $quantity ->
        [one] no se pudo revisar { $count } día
        [many] no se pudieron revisar { $count } días
       *[other] no se pudieron revisar { $count } días
    }

journal.window.health.label.review.folder.privacy = revisar la privacidad de la carpeta

journal.window.health.label.view.backup.details = ver detalles de la copia de seguridad

journal.window.health.label.view.devices = ver dispositivos

journal.window.health.label.view.queued.work = ver el trabajo en cola

journal.window.health.link.label.get.help = pedir ayuda →

journal.window.health.link.label.manage.backup = administrar la copia de seguridad →

journal.window.health.link.label.view.devices = ver dispositivos →

journal.window.health.local.program.requests =
    { $quantity ->
        [one] { $program } · { $count } solicitud · { $last_seen }
        [many] { $program } · { $count } solicitudes · { $last_seen }
       *[other] { $program } · { $count } solicitudes · { $last_seen }
    }

journal.window.health.logs.a.required.field.is.missing = falta un campo obligatorio.

journal.window.health.logs.errors.count =
    { $quantity ->
        [one] { $count } error
        [many] { $count } errores
       *[other] { $count } errores
    }

journal.window.health.logs.failed.to.read.log.file = No se pudo leer el archivo de registro

journal.window.health.logs.invalid.path = Ruta no válida

journal.window.health.logs.labels.logs.level.filter.level = nivel

journal.window.health.logs.labels.logs.level.option.all.levels = todos los niveles

journal.window.health.logs.labels.logs.level.option.errors.only = solo errores

journal.window.health.logs.labels.logs.level.option.info.above = info y superior

journal.window.health.logs.labels.logs.level.option.warnings.errors = advertencias y errores

journal.window.health.logs.labels.logs.service.filter.service = servicio

journal.window.health.logs.labels.logs.stream.filter.stream = flujo

journal.window.health.logs.lines.count =
    { $quantity ->
        [one] 1 línea
        [many] { $count } líneas
       *[other] { $count } líneas
    }

journal.window.health.logs.log.file.not.found = No se encontró el archivo de registro

journal.window.health.logs.missing.path.parameter = Falta el parámetro { $slot1 }

journal.window.health.logs.service.collapsed =
    { $line_quantity ->
        [one] ── { $service } ── ({ $lines } línea, ★ { $error_quantity ->
        [one] { $errors } error
        [many] { $errors } errores
       *[other] { $errors } errores
    })
        [many] ── { $service } ── ({ $lines } líneas, ★ { $error_quantity ->
        [one] { $errors } error
        [many] { $errors } errores
       *[other] { $errors } errores
    })
       *[other] ── { $service } ── ({ $lines } líneas, ★ { $error_quantity ->
        [one] { $errors } error
        [many] { $errors } errores
       *[other] { $errors } errores
    })
    }

journal.window.health.logs.that.file.couldn.t.be.read = no se pudo leer ese archivo.

journal.window.health.logs.that.file.isn.t.available = ese archivo no está disponible.

journal.window.health.logs.that.path.couldn.t.be.used = no se pudo usar esa ruta.

journal.window.health.observe.local.reporting = la computadora de tu diario todavía no se reporta. mostrando { $stream }.

journal.window.health.observe.unknown.reporting = todavía no se puede identificar la computadora de tu diario. mostrando { $stream }.

journal.window.health.option.label.all.services = todos los servicios

journal.window.health.option.label.all.streams = todos los flujos

journal.window.health.option.label.log = registro

journal.window.health.option.label.stderr = stderr

journal.window.health.option.label.stdout = stdout

journal.window.health.report.failed = No se pudo completar el informe de estado

journal.window.health.report.invalid = Valor de solicitud no válido

journal.window.health.report.missing = Falta un campo obligatorio

journal.window.health.report.unavailable = informe de estado no disponible

journal.window.health.schedule.names.brain.processing.check = revisión del procesamiento

journal.window.health.schedule.names.cadence.processing.schedule = horario de procesamiento

journal.window.health.schedule.names.facet.candidates.facet.suggestions = sugerencias de facetas

journal.window.health.schedule.names.heartbeat.journal.review = revisión del diario

journal.window.health.schedule.names.maintenance.backup.offload.original.media.cleanup = limpieza de contenido multimedia original

journal.window.health.schedule.names.maintenance.backup.prune.backup.cleanup = limpieza de la copia de seguridad

journal.window.health.schedule.names.maintenance.backup.run.backup = copia de seguridad

journal.window.health.schedule.names.maintenance.backup.verify.backup.check = revisión de la copia de seguridad

journal.window.health.schedule.names.maintenance.health.mark.raw.original.media.review = revisión de contenido multimedia original

journal.window.health.schedule.names.maintenance.health.prune.logs.log.cleanup = limpieza de registros

journal.window.health.schedule.names.maintenance.retention.owner.voice.audio.deletion.catch.up = puesta al día de la eliminación de audio

journal.window.health.schedule.names.maintenance.speakers.backfill.labels.speaker.naming.catch.up = puesta al día de los nombres de hablantes

journal.window.health.schedule.names.maintenance.speakers.backfill.pool.voice.list.catch.up = puesta al día de la lista de voces

journal.window.health.schedule.names.maintenance.speakers.candidate.pair.speaker.suggestions = sugerencias de hablantes

journal.window.health.schedule.names.maintenance.speakers.consolidate.pool.speaker.cleanup = limpieza de hablantes

journal.window.health.schedule.names.maintenance.speakers.discover.voices.voice.discovery = descubrimiento de voces

journal.window.health.schedule.names.maintenance.speakers.name.variants.speaker.name.suggestions = sugerencias de nombres de hablantes

journal.window.health.schedule.names.maintenance.trash.erase.trash.erase = borrado de la papelera

journal.window.health.schedule.next.duration = en { $duration }

journal.window.health.schedule.next.named = siguiente: { $name } { $when }

journal.window.health.schedule.next.unnamed = próxima ejecución programada { $when }

journal.window.health.schedule.summary.due =
    { $due_quantity ->
        [one] { $scheduled_quantity ->
        [one] { $count } programado
        [many] { $count } programados
       *[other] { $count } programados
    } · uno está pendiente ahora
        [many] { $scheduled_quantity ->
        [one] { $count } programado
        [many] { $count } programados
       *[other] { $count } programados
    } · { $due } están pendientes ahora
       *[other] { $scheduled_quantity ->
        [one] { $count } programado
        [many] { $count } programados
       *[other] { $count } programados
    } · { $due } están pendientes ahora
    }

journal.window.health.schedule.summary.idle =
    { $quantity ->
        [one] { $count } programado
        [many] { $count } programados
       *[other] { $count } programados
    }

journal.window.health.schedule.summary.next =
    { $quantity ->
        [one] { $count } programado · siguiente { $when }
        [many] { $count } programados · siguiente { $when }
       *[other] { $count } programados · siguiente { $when }
    }

journal.window.health.search.last.check.elapsed =
    { $hour_quantity ->
        [one] última revisión de búsqueda hace { $hours } hora: { $updated_quantity ->
        [one] { $updated } actualizado
        [many] { $updated } actualizados
       *[other] { $updated } actualizados
    }, { $removed_quantity ->
        [one] { $removed } quitado
        [many] { $removed } quitados
       *[other] { $removed } quitados
    }
        [many] última revisión de búsqueda hace { $hours } horas: { $updated_quantity ->
        [one] { $updated } actualizado
        [many] { $updated } actualizados
       *[other] { $updated } actualizados
    }, { $removed_quantity ->
        [one] { $removed } quitado
        [many] { $removed } quitados
       *[other] { $removed } quitados
    }
       *[other] última revisión de búsqueda hace { $hours } horas: { $updated_quantity ->
        [one] { $updated } actualizado
        [many] { $updated } actualizados
       *[other] { $updated } actualizados
    }, { $removed_quantity ->
        [one] { $removed } quitado
        [many] { $removed } quitados
       *[other] { $removed } quitados
    }
    }

journal.window.health.search.last.check.recent =
    { $updated_quantity ->
        [one] última revisión de búsqueda hace menos de una hora: { $updated } actualizado, { $removed_quantity ->
        [one] { $removed } quitado
        [many] { $removed } quitados
       *[other] { $removed } quitados
    }
        [many] última revisión de búsqueda hace menos de una hora: { $updated } actualizados, { $removed_quantity ->
        [one] { $removed } quitado
        [many] { $removed } quitados
       *[other] { $removed } quitados
    }
       *[other] última revisión de búsqueda hace menos de una hora: { $updated } actualizados, { $removed_quantity ->
        [one] { $removed } quitado
        [many] { $removed } quitados
       *[other] { $removed } quitados
    }
    }

journal.window.health.search.queued =
    { $quantity ->
        [one] { $count } actualización en cola
        [many] { $count } actualizaciones en cola
       *[other] { $count } actualizaciones en cola
    }

journal.window.health.search.removals =
    { $quantity ->
        [one] { $count } todavía pendiente de quitar
        [many] { $count } todavía pendientes de quitar
       *[other] { $count } todavía pendientes de quitar
    }

journal.window.health.search.summary.details = { $status } { $details }.

journal.window.health.section.heading.backup = copia de seguridad

journal.window.health.section.heading.checking.device.activity = revisando la actividad del dispositivo…

journal.window.health.section.heading.journal.status.details = detalles del estado del diario

journal.window.health.section.heading.processing = procesamiento

journal.window.health.section.heading.programs.using.the.older.local.connection = programas que usan la conexión local anterior

journal.window.health.section.heading.right.now = ahora mismo

journal.window.health.section.heading.set.aside.work = trabajo apartado

journal.window.health.section.heading.work.in.your.journal = trabajo en tu diario

journal.window.health.service.names.convey.web.interface = interfaz web

journal.window.health.service.names.cortex.model.runtime = entorno de ejecución del modelo

journal.window.health.service.names.importer.file.importer = importador de archivos

journal.window.health.service.names.observe.screen.audio = pantalla y audio

journal.window.health.service.names.parakeet.transcription = transcripción

journal.window.health.service.names.schedule.task.scheduler = programador de tareas

journal.window.health.service.names.sense.media.processor = procesador de contenido multimedia

journal.window.health.service.names.spl.private.network = red privada

journal.window.health.service.names.supervisor.system.manager = administrador del sistema

journal.window.health.service.names.think.background.analysis = análisis en segundo plano

journal.window.health.speech.setup.copy.explanation.automatic.setup.is.waiting.to.try = la configuración inicial automática espera para volver a intentarlo.

journal.window.health.speech.setup.copy.explanation.local.speech.recognition.isn.t.ready = el reconocimiento de voz local no está listo.

journal.window.health.speech.setup.copy.explanation.setup.hasn.t.started.yet = la configuración inicial todavía no empezó.

journal.window.health.speech.setup.copy.explanation.setup.was.cancelled = se canceló la configuración inicial.

journal.window.health.speech.setup.copy.explanation.speech.recognition.setup.couldn.t.be = no se pudo revisar la configuración inicial del reconocimiento de voz.

journal.window.health.speech.setup.copy.state.checking.the.download = revisando la descarga

journal.window.health.speech.setup.copy.state.downloading = descargando

journal.window.health.speech.setup.copy.state.installing = instalando

journal.window.health.speech.setup.copy.state.setup.couldn.t.finish = no se pudo terminar la configuración inicial

journal.window.health.speech.setup.copy.state.setup.needs.attention = la configuración inicial necesita atención

journal.window.health.speech.setup.copy.state.setup.paused = configuración inicial en pausa

journal.window.health.speech.setup.copy.state.status.unavailable = estado no disponible

journal.window.health.speech.setup.copy.state.waiting.for.setup = en espera de la configuración inicial

journal.window.health.speech.setup.copy.summary.setup.details = detalles de la configuración inicial

journal.window.health.speech.setup.copy.summary.show.recovery.steps = mostrar los pasos de recuperación

journal.window.health.speech.setup.download.bytes = { $received } de { $total }

journal.window.health.status.active = activo

journal.window.health.status.activity.processing = procesamiento de actividad

journal.window.health.status.adding = agregando

journal.window.health.status.all.queued.work = todo el trabajo en cola

journal.window.health.status.an.unnamed.device = un dispositivo sin nombre

journal.window.health.status.audio.muted = audio silenciado

journal.window.health.status.checking = revisando

journal.window.health.status.crashed = falló

journal.window.health.status.daily.processing = procesamiento diario

journal.window.health.status.day = día

journal.window.health.status.default = predeterminado

journal.window.health.status.delivery.unknown = entrega desconocida

journal.window.health.status.didn.t.finish = no terminó

journal.window.health.status.didn.t.finish.importing = no terminó de importar

journal.window.health.status.due.now = vence ahora

journal.window.health.status.facet = faceta

journal.window.health.status.finishing.activities = terminando actividades

journal.window.health.status.finishing.older.days = terminando días anteriores

journal.window.health.status.idle = reposo

journal.window.health.status.import = importación

journal.window.health.status.imports = importaciones

journal.window.health.status.just.now = justo ahora

journal.window.health.status.loading = cargando

journal.window.health.status.loading.variant2 = cargando…

journal.window.health.status.mode = modo

journal.window.health.status.never.reported = nunca se reportó

journal.window.health.status.no.devices = no hay dispositivos

journal.window.health.status.no.recorded.days = no hay días registrados

journal.window.health.status.ok = ok

journal.window.health.status.organizing.segments = organizando segmentos…

journal.window.health.status.power.saving = ahorro de energía

journal.window.health.status.processing = procesando…

journal.window.health.status.queued.processing = procesamiento en cola

journal.window.health.status.quiet = en silencio

journal.window.health.status.recent.days = días recientes

journal.window.health.status.retrying = reintentando

journal.window.health.status.running.variant2 = en ejecución…

journal.window.health.status.screen.locked = pantalla bloqueada

journal.window.health.status.search.updates = actualizaciones de la búsqueda

journal.window.health.status.segment = segmento

journal.window.health.status.segment.processing = procesamiento de segmento

journal.window.health.status.setup.details = detalles de la configuración inicial

journal.window.health.status.starting = iniciando…

journal.window.health.status.today = hoy

journal.window.health.status.transcribing.audio = transcribiendo el audio…

journal.window.health.status.unavailable = no disponible

journal.window.health.status.under.a.minute = menos de un minuto

journal.window.health.status.unknown = desconocido

journal.window.health.status.unnamed = sin nombre

journal.window.health.status.updating.older.days = actualizando días anteriores

journal.window.health.status.uploads.rejected = subidas rechazadas

journal.window.health.status.warning.services.retrying = advertencia, los servicios están reintentando

journal.window.health.status.weekly.reflection = reflexión semanal

journal.window.health.status.working = trabajando…

journal.window.health.storage.privacy.accept = conservarlo en esta unidad

journal.window.health.storage.privacy.accepted = elegiste conservar tu diario en esta unidad, donde otras cuentas en esta computadora podrían leerlo.

journal.window.health.storage.privacy.changed = cambió el almacenamiento de tu diario. vuelve a cargar esta página antes de elegir, por favor.

journal.window.health.storage.privacy.heading = privacidad de la carpeta del diario

journal.window.health.storage.privacy.private = la carpeta de tu diario es privada para tu cuenta en esta computadora.

journal.window.health.storage.privacy.save.failed = no se pudo guardar tu elección. vuelve a intentarlo, por favor.

journal.window.health.storage.privacy.unavailable = no se pudo revisar la privacidad de la carpeta de tu diario.

journal.window.health.storage.privacy.warning = otras cuentas en esta computadora podrían leer tu diario. su carpeta no se pudo dejar privada en esta unidad. puedes conservarlo en esta unidad o moverlo a una unidad que mantiene privadas las carpetas.

journal.window.health.talent.names.entities.detection.entity.detection = detección de entidades

journal.window.health.talent.names.entities.entity.observer.entity.facts = hechos de la entidad

journal.window.health.talent.names.entities.possible.names.possible.names = nombres posibles

journal.window.health.talent.names.screen.screen.descriptions = descripciones de pantalla

journal.window.health.text.checking.backup.status = revisando el estado de la copia de seguridad…

journal.window.health.thinking.brain.checked = { $lane }, se revisó { $age }

journal.window.health.thinking.brain.reason = { $lane }: { $reason }

journal.window.health.thinking.check.progress.runs = ejecuciones: { $completed } / { $total }

journal.window.health.thinking.check.progress.segments = segmentos: { $completed } / { $total }

journal.window.health.thinking.check.running = en ejecución: { $agents }

journal.window.health.thinking.last.run.finished = la última ejecución terminó { $age }

journal.window.health.thinking.recent.errors =
    { $quantity ->
        [one] { $count } error reciente
        [many] { $count } errores recientes
       *[other] { $count } errores recientes
    }

journal.window.health.thinking.tokens.today =
    { $quantity ->
        [one] { $count } token usado hoy
        [many] { $count } tokens usados hoy
       *[other] { $count } tokens usados hoy
    }

journal.window.health.time.legacy.age = hace { $duration }

journal.window.health.visible.text.active.imports = Importaciones activas

journal.window.health.visible.text.active.runs = Ejecuciones activas

journal.window.health.visible.text.all.services.running = todos los servicios en ejecución

journal.window.health.visible.text.audio = audio

journal.window.health.visible.text.audio.transcription = transcripción de audio

journal.window.health.visible.text.background.analysis = análisis en segundo plano

journal.window.health.visible.text.check.again = revisar de nuevo

journal.window.health.visible.text.checking.backup.status = revisando el estado de la copia de seguridad…

journal.window.health.visible.text.checking.journal.backlog = revisando el trabajo pendiente del diario…

journal.window.health.visible.text.checking.processing = revisando el procesamiento…

journal.window.health.visible.text.checking.this.work = revisando este trabajo…

journal.window.health.visible.text.checking.what.is.left = revisando lo que queda…

journal.window.health.visible.text.clear = limpiar

journal.window.health.visible.text.clock.skew = desfase del reloj

journal.window.health.visible.text.couldn.t.check.thinking.errors.today = hoy no se pudo revisar los errores de razonamiento.

journal.window.health.visible.text.default = predeterminado

journal.window.health.visible.text.dismiss = descartar

journal.window.health.visible.text.error = error

journal.window.health.visible.text.error.today = error hoy

journal.window.health.visible.text.errors.today = errores hoy

journal.window.health.visible.text.everything.is.idle = todo está en reposo

journal.window.health.visible.text.finishing.older.recordings.and.day.updates = terminar grabaciones anteriores y actualizaciones del día usa el modelo que elegiste y puede añadir cargos de tu proveedor. el trabajo de hoy va primero.

journal.window.health.visible.text.health = estado

journal.window.health.visible.text.hide = ▼ ocultar

journal.window.health.visible.text.idle = reposo

journal.window.health.visible.text.journal.source = fuente del diario

journal.window.health.visible.text.local.speech.recognition = reconocimiento de voz local

journal.window.health.visible.text.log.file = archivo de registro

journal.window.health.visible.text.log.updates.may.be.delayed = las actualizaciones del registro pueden demorarse

journal.window.health.visible.text.no.matching.recent.errors.yet = todavía no hay errores recientes que coincidan.

journal.window.health.visible.text.no.recent.errors = no hay errores recientes

journal.window.health.visible.text.no.recent.errors.variant2 = no hay errores recientes.

journal.window.health.visible.text.none.since.this.journal.started = ninguno desde que empezó este diario.

journal.window.health.visible.text.ok = ok

journal.window.health.visible.text.older.work.is.allowed.to.use = se permite que el trabajo anterior use el modelo que elegiste, que puede añadir cargos de tu proveedor. el trabajo de hoy va primero.

journal.window.health.visible.text.on.the.computer.running.your.journal = en la computadora donde se ejecuta tu diario:

journal.window.health.visible.text.processing.details = detalles del procesamiento

journal.window.health.visible.text.processing.status.unavailable = estado del procesamiento no disponible.

journal.window.health.visible.text.programs.that.reach.your.journal.without = programas que conectan con tu diario sin un navegador, desde que empezó tu diario. esta lista se queda en esta computadora.

journal.window.health.visible.text.queued.work.status.is.unavailable = el estado del trabajo en cola no está disponible.

journal.window.health.visible.text.queues = colas

journal.window.health.visible.text.recent.errors = Errores recientes

journal.window.health.visible.text.retry = reintentar

journal.window.health.visible.text.retry.failed = falló el reintento

journal.window.health.visible.text.retry.sent = reintento enviado

journal.window.health.visible.text.retrying = reintentando…

journal.window.health.visible.text.run.id = id de la ejecución

journal.window.health.visible.text.running = en ejecución

journal.window.health.visible.text.saving.your.choice = guardando tu elección…

journal.window.health.visible.text.scene.analysis = análisis de escena

journal.window.health.visible.text.schedules = programaciones

journal.window.health.visible.text.screen = pantalla

journal.window.health.visible.text.service.logs = registros del servicio

journal.window.health.visible.text.service.status.unavailable = estado del servicio no disponible

journal.window.health.visible.text.services = servicios

journal.window.health.visible.text.services.need.attention = los servicios necesitan atención

journal.window.health.visible.text.services.retrying = los servicios están reintentando

journal.window.health.visible.text.show = ▶ mostrar

journal.window.health.visible.text.some.services.slow.to.respond = algunos servicios tardan en responder

journal.window.health.visible.text.sources.adding.to.your.journal = fuentes que se agregan a tu diario

journal.window.health.visible.text.stale = desactualizado

journal.window.health.visible.text.system.activity = actividad del sistema

journal.window.health.visible.text.system.vitals = signos vitales del sistema

journal.window.health.visible.text.taking.in.your.screen = recibiendo tu pantalla

journal.window.health.visible.text.tasks.variant2 = tareas

journal.window.health.visible.text.technical.details = detalles técnicos

journal.window.health.visible.text.terminal.sessions = sesiones de terminal

journal.window.health.visible.text.the.journal.has.not.finished.checking = el diario no ha terminado de revisar su trabajo. mantenlo en ejecución para ver el avance aquí.

journal.window.health.visible.text.the.retry.could.not.be.saved = no se pudo guardar el reintento. vuelve a intentarlo.

journal.window.health.visible.text.thinking = razonamiento

journal.window.health.visible.text.this.list.could.not.be.read = no se pudo leer esta lista. vuelve a intentarlo pronto.

journal.window.health.visible.text.this.work.could.not.be.retried = no se pudo reintentar este trabajo.

journal.window.health.visible.text.try.again = reintentar

journal.window.health.visible.text.unavailable = no disponible

journal.window.health.visible.text.unknown = desconocido

journal.window.health.visible.text.unnamed.device = dispositivo sin nombre

journal.window.health.visible.text.updates.are.slow = las actualizaciones están lentas

journal.window.health.visible.text.view.logs = ver registros

journal.window.health.visible.text.waiting = en espera…

journal.window.health.visible.text.waiting.for.live.updates = en espera de actualizaciones en vivo

journal.window.health.visible.text.warning = advertencia

journal.window.health.visible.text.what.s.going.into.your.journal = qué está recibiendo tu diario

journal.window.health.visible.text.your.choice.could.not.be.saved = no se pudo guardar tu elección. vuelve a intentarlo.

journal.window.health.visible.text.your.choice.could.not.be.saved.variant2 = no se pudo guardar tu elección. vuelve a intentarlo, por favor.

journal.window.health.visible.text.your.devices = tus dispositivos

journal.window.health.visible.text.your.request.is.saved = tu solicitud está guardada.

journal.window.health.vitals.connection.quiet.since = sin actualizaciones en { $duration }

journal.window.health.vitals.connection.updated = se actualizó { $age }

journal.window.health.vitals.health.accessible = estado: { $status }

journal.window.health.vitals.heartbeat.stopped =
    { $quantity ->
        [one] { $warning_start }{ $count } ejecución del diario dejó de reportarse:{ $warning_end }{ $runs_start } { $run_names }{ $runs_end }
        [many] { $warning_start }{ $count } ejecuciones del diario dejaron de reportarse:{ $warning_end }{ $runs_start } { $run_names }{ $runs_end }
       *[other] { $warning_start }{ $count } ejecuciones del diario dejaron de reportarse:{ $warning_end }{ $runs_start } { $run_names }{ $runs_end }
    }

journal.window.health.vitals.heartbeat.warning =
    { $quantity ->
        [one] advertencia, 1 ejecución del diario dejó de reportarse
        [many] advertencia, { $count } ejecuciones del diario dejaron de reportarse
       *[other] advertencia, { $count } ejecuciones del diario dejaron de reportarse
    }

journal.window.health.vitals.queues.accessible = colas: { $status }

journal.window.health.vitals.schedules.accessible = programaciones: { $status }

journal.window.health.vitals.schedules.count =
    { $quantity ->
        [one] { $count } programado
        [many] { $count } programados
       *[other] { $count } programados
    }

journal.window.health.vitals.service.active = { $service }: en activo

journal.window.health.vitals.service.crashed = { $service }: falló

journal.window.health.vitals.service.retrying = { $service }: reintentando (reintentando)

journal.window.health.vitals.service.retrying.label = { $service } (reintentando)

journal.window.health.vitals.services.accessible = servicios: { $status }

journal.window.health.vitals.services.active =
    { $quantity ->
        [one] { $count } en activo
        [many] { $count } en activo
       *[other] { $count } en activo
    }

journal.window.health.vitals.services.attention =
    { $quantity ->
        [one] { $count } necesita atención
        [many] { $count } necesitan atención
       *[other] { $count } necesitan atención
    }

journal.window.health.vitals.services.retrying =
    { $quantity ->
        [one] { $count } reintentando
        [many] { $count } reintentando
       *[other] { $count } reintentando
    }

journal.window.health.vitals.tasks.accessible = tareas: { $status }

journal.window.health.vitals.tasks.active =
    { $quantity ->
        [one] { $count } en activo
        [many] { $count } en activo
       *[other] { $count } en activo
    }

journal.window.health.vitals.thinking.accessible = razonamiento: { $status }

journal.window.health.vitals.thinking.running =
    { $quantity ->
        [one] { $count } en ejecución
        [many] { $count } en ejecución
       *[other] { $count } en ejecución
    }

journal.window.health.window.explanation.analysis.has.failed.repeatedly = el análisis falló varias veces.

journal.window.health.window.explanation.choose.a.model.in.thinking.before = elige un modelo en razonamiento antes de que este trabajo pueda ejecutarse.

journal.window.health.window.explanation.error.network.error.loading.log.file = ERROR: error de red al cargar el archivo de registro

journal.window.health.window.explanation.error.services.need.attention = error, los servicios necesitan atención

journal.window.health.window.explanation.in.under.a.minute = en menos de un minuto

journal.window.health.window.explanation.last.added.from.future = último agregado desde el futuro

journal.window.health.window.explanation.last.seen.from.future = visto por última vez desde el futuro

journal.window.health.window.explanation.network.error.loading.log.file = error de red al cargar el archivo de registro

journal.window.health.window.explanation.no.finished.search.check.yet = todavía no hay una revisión terminada de la búsqueda

journal.window.health.window.explanation.no.older.history.to.check = no hay historial anterior para revisar

journal.window.health.window.explanation.older.history.is.still.catching.up = el historial anterior todavía se está poniendo al día

journal.window.health.window.explanation.ran.into.a.problem = se encontró un problema

journal.window.health.window.explanation.reload.to.try.again = recarga para volver a intentarlo.

journal.window.health.window.explanation.the.model.repeatedly.returned.an.unusable = el modelo devolvió varias veces un resultado que no se puede usar.

journal.window.health.window.explanation.the.recording.could.not.be.read = no se pudo leer la grabación. reemplaza su fuente antes de volver a intentarlo.

journal.window.health.window.explanation.the.recording.has.no.audio.stream = la grabación no tiene un flujo de audio.

journal.window.health.window.explanation.this.processing.step.has.been.set = este paso del procesamiento quedó apartado.

journal.window.health.window.explanation.this.recording.exceeds.the.audio.processing = esta grabación supera el límite de procesamiento de audio en este dispositivo. se conserva el original. usa una grabación más corta, reduce sus pistas de audio o procésala en una computadora.

journal.window.health.window.explanation.this.recording.is.larger.than.the = esta grabación es más grande que el límite de 16 mib en este dispositivo. se conserva el original. usa una grabación más pequeña o procésala en una computadora.

journal.window.health.window.explanation.waiting.before.another.attempt = en espera antes de otro intento.

journal.window.health.window.explanation.waiting.for.permission.to.process.older = en espera de permiso para procesar el trabajo anterior.

journal.window.health.window.explanation.waiting.for.the.recordings.on.this = en espera de las grabaciones de este día.

journal.window.health.window.explanation.your.requests.and.yesterday = tus solicitudes y ayer

journal.window.health.work.band.activities =
    { $quantity ->
        [one] { $count } actividad restante
        [many] { $count } actividades restantes
       *[other] { $count } actividades restantes
    }

journal.window.health.work.band.owner.requests =
    { $quantity ->
        [one] { $count } solicitud para rehacer un día
        [many] { $count } solicitudes para rehacer un día
       *[other] { $count } solicitudes para rehacer un día
    }

journal.window.health.work.band.permission =
    { $quantity ->
        [one] { $count } en espera de permiso
        [many] { $count } en espera de permiso
       *[other] { $count } en espera de permiso
    }

journal.window.health.work.band.progress =
    { $recordings_ready_quantity ->
        [one] { $band }: { $recordings_ready } grabación lista, { $recordings_left_quantity ->
        [one] { $recordings_left } restante
        [many] { $recordings_left } restantes
       *[other] { $recordings_left } restantes
    }; { $outputs_ready_quantity ->
        [one] { $outputs_ready } resultado diario listo
        [many] { $outputs_ready } resultados diarios listos
       *[other] { $outputs_ready } resultados diarios listos
    }, { $outputs_left_quantity ->
        [one] { $outputs_left } restante
        [many] { $outputs_left } restantes
       *[other] { $outputs_left } restantes
    }.
        [many] { $band }: { $recordings_ready } grabaciones listas, { $recordings_left_quantity ->
        [one] { $recordings_left } restante
        [many] { $recordings_left } restantes
       *[other] { $recordings_left } restantes
    }; { $outputs_ready_quantity ->
        [one] { $outputs_ready } resultado diario listo
        [many] { $outputs_ready } resultados diarios listos
       *[other] { $outputs_ready } resultados diarios listos
    }, { $outputs_left_quantity ->
        [one] { $outputs_left } restante
        [many] { $outputs_left } restantes
       *[other] { $outputs_left } restantes
    }.
       *[other] { $band }: { $recordings_ready } grabaciones listas, { $recordings_left_quantity ->
        [one] { $recordings_left } restante
        [many] { $recordings_left } restantes
       *[other] { $recordings_left } restantes
    }; { $outputs_ready_quantity ->
        [one] { $outputs_ready } resultado diario listo
        [many] { $outputs_ready } resultados diarios listos
       *[other] { $outputs_ready } resultados diarios listos
    }, { $outputs_left_quantity ->
        [one] { $outputs_left } restante
        [many] { $outputs_left } restantes
       *[other] { $outputs_left } restantes
    }.
    }

journal.window.health.work.band.progress.details =
    { $recordings_ready_quantity ->
        [one] { $band }: { $recordings_ready } grabación lista, { $recordings_left_quantity ->
        [one] { $recordings_left } restante
        [many] { $recordings_left } restantes
       *[other] { $recordings_left } restantes
    }; { $outputs_ready_quantity ->
        [one] { $outputs_ready } resultado diario listo
        [many] { $outputs_ready } resultados diarios listos
       *[other] { $outputs_ready } resultados diarios listos
    }, { $outputs_left_quantity ->
        [one] { $outputs_left } restante
        [many] { $outputs_left } restantes
       *[other] { $outputs_left } restantes
    }; { $details }.
        [many] { $band }: { $recordings_ready } grabaciones listas, { $recordings_left_quantity ->
        [one] { $recordings_left } restante
        [many] { $recordings_left } restantes
       *[other] { $recordings_left } restantes
    }; { $outputs_ready_quantity ->
        [one] { $outputs_ready } resultado diario listo
        [many] { $outputs_ready } resultados diarios listos
       *[other] { $outputs_ready } resultados diarios listos
    }, { $outputs_left_quantity ->
        [one] { $outputs_left } restante
        [many] { $outputs_left } restantes
       *[other] { $outputs_left } restantes
    }; { $details }.
       *[other] { $band }: { $recordings_ready } grabaciones listas, { $recordings_left_quantity ->
        [one] { $recordings_left } restante
        [many] { $recordings_left } restantes
       *[other] { $recordings_left } restantes
    }; { $outputs_ready_quantity ->
        [one] { $outputs_ready } resultado diario listo
        [many] { $outputs_ready } resultados diarios listos
       *[other] { $outputs_ready } resultados diarios listos
    }, { $outputs_left_quantity ->
        [one] { $outputs_left } restante
        [many] { $outputs_left } restantes
       *[other] { $outputs_left } restantes
    }; { $details }.
    }

journal.window.health.work.band.recordings =
    { $quantity ->
        [one] { $count } en espera de grabaciones
        [many] { $count } en espera de grabaciones
       *[other] { $count } en espera de grabaciones
    }

journal.window.health.work.hold.page = página { $page } de { $pages }

journal.window.health.work.hold.reason = { $item }: { $reason }

journal.window.health.work.hold.reason.count =
    { $quantity ->
        [one] { $item } · { $count } elemento: { $reason }
        [many] { $item } · { $count } elementos: { $reason }
       *[other] { $item } · { $count } elementos: { $reason }
    }

journal.window.health.work.hold.stale = el trabajo apartado se revisó por última vez { $checked_at }. revisando su estado de nuevo.

journal.window.health.work.hold.tasks.current =
    { $quantity ->
        [one] { $scope }: { $count } tarea en espera de una revisión de seguridad.
        [many] { $scope }: { $count } tareas en espera de una revisión de seguridad.
       *[other] { $scope }: { $count } tareas en espera de una revisión de seguridad.
    }

journal.window.health.work.hold.tasks.last.check =
    { $quantity ->
        [one] { $scope }: { $count } tarea en espera en la última revisión.
        [many] { $scope }: { $count } tareas en espera en la última revisión.
       *[other] { $scope }: { $count } tareas en espera en la última revisión.
    }

journal.window.health.work.hold.unavailable = el estado del trabajo en cola no está disponible. último estado conocido, revisado { $checked_at }.

journal.window.health.work.inspection.complete =
    { $quantity ->
        [one] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } día revisado ({ $range }). la revisión cubre todo el diario.
        [many] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } días revisados ({ $range }). la revisión cubre todo el diario.
       *[other] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } días revisados ({ $range }). la revisión cubre todo el diario.
    }

journal.window.health.work.inspection.incomplete =
    { $quantity ->
        [one] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } día revisado ({ $range }). la revisión está incompleta.
        [many] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } días revisados ({ $range }). la revisión está incompleta.
       *[other] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } días revisados ({ $range }). la revisión está incompleta.
    }

journal.window.health.work.inspection.stale =
    { $quantity ->
        [one] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } día revisado ({ $range }). último avance conocido, revisado { $checked_at }.
        [many] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } días revisados ({ $range }). último avance conocido, revisado { $checked_at }.
       *[other] { $checked_quantity ->
        [one] { $checked }
        [many] { $checked }
       *[other] { $checked }
    } de { $total } días revisados ({ $range }). último avance conocido, revisado { $checked_at }.
    }

journal.window.notes.transcript.edited = editada

journal.window.notes.transcript.removed = ya no está en la bóveda

journal.window.notes.transcript.removed.words =
    { $quantity ->
        [one] se quitó { $count } palabra.
        [many] se quitaron { $count } palabras.
       *[other] se quitaron { $count } palabras.
    }

journal.window.notes.transcript.renamed = renombrado desde { $path }

journal.window.notes.transcript.source = Fuente: { $path }

journal.window.speakers.accessible.name.apply.assignment = aplicar la asignación

journal.window.speakers.accessible.name.apply.correction = aplicar la corrección

journal.window.speakers.accessible.name.assign.speaker = asignar hablante

journal.window.speakers.accessible.name.correct.attribution = corregir la atribución

journal.window.speakers.accessible.name.dismiss = descartar

journal.window.speakers.accessible.name.find.a.voice.by.name = buscar una voz por nombre

journal.window.speakers.accessible.name.segments = segmentos

journal.window.speakers.accessible.name.sort.voices = ordenar voces

journal.window.speakers.attribution.attribution.response.a.required.field.is.missing = falta un campo obligatorio.

journal.window.speakers.attribution.attribution.response.detail.attribution.is.not.medium.confidence = la atribución no es de confianza media

journal.window.speakers.attribution.attribution.response.detail.choose.two.different.speakers = Elige dos hablantes distintos.

journal.window.speakers.attribution.attribution.response.detail.entity.speaker.is.hidden = La entidad '{ $speaker }' está oculta

journal.window.speakers.attribution.attribution.response.detail.entity.speaker.is.not.a.person = La entidad '{ $speaker }' no es una Persona. Selecciona una Persona existente que no esté oculta.

journal.window.speakers.attribution.attribution.response.detail.entity.speaker.not.found = No se encontró la entidad '{ $speaker }'

journal.window.speakers.attribution.attribution.response.detail.invalid.day.format = Formato de día no válido

journal.window.speakers.attribution.attribution.response.detail.invalid.segment.key.or.stream = Clave de segmento no válida, o flujo no válido

journal.window.speakers.attribution.attribution.response.detail.invalid.stream = Flujo no válido

journal.window.speakers.attribution.attribution.response.detail.invalid.stream.layout = Diseño del flujo no válido

journal.window.speakers.attribution.attribution.response.detail.missing.required.fields = Faltan campos obligatorios

journal.window.speakers.attribution.attribution.response.detail.no.data.provided = No se proporcionaron datos

journal.window.speakers.attribution.attribution.response.detail.no.speaker.labels.found = No se encontraron etiquetas de hablante

journal.window.speakers.attribution.attribution.response.detail.pick.a.different.sentence.with.an = Elige otra oración que tenga una incrustación.

journal.window.speakers.attribution.attribution.response.detail.pick.a.sentence.without.a.speaker = Elige una oración sin hablante.

journal.window.speakers.attribution.attribution.response.detail.regex.input.was.not.a.string = la entrada de regex no era una cadena

journal.window.speakers.attribution.attribution.response.detail.request.body.must.be.a.json = el cuerpo de la solicitud debe ser un objeto JSON

journal.window.speakers.attribution.attribution.response.detail.sentence.embedding.not.found = No se encontró la incrustación de la oración

journal.window.speakers.attribution.attribution.response.detail.sentence.has.no.speaker.assignment.yet = la oración todavía no tiene hablante asignado

journal.window.speakers.attribution.attribution.response.detail.sentence.not.found.in.labels = No se encontró la oración en las etiquetas

journal.window.speakers.attribution.attribution.response.detail.source.was.not.a.string = la fuente no era una cadena

journal.window.speakers.attribution.attribution.response.detail.unknown.owner.contamination.reason.reason = motivo desconocido de contaminación del propietario:{ $slot1 }{ $reason }

journal.window.speakers.attribution.attribution.response.detail.use.a.valid.day.stream.and = Usa un día, un flujo y un segmento válidos, y luego elige una oración.

journal.window.speakers.attribution.attribution.response.detail.use.one.file.name.without.a = usa un solo nombre de archivo, sin ruta

journal.window.speakers.attribution.attribution.response.one.of.those.values.couldn.t = no se pudo usar uno de esos valores.

journal.window.speakers.attribution.attribution.response.source.name.is.invalid = el nombre de la fuente no es válido.

journal.window.speakers.attribution.attribution.response.that.change.couldn.t.be.applied = no se pudo aplicar ese cambio porque la oración no está en el estado correcto.

journal.window.speakers.attribution.attribution.response.that.day.couldn.t.be.used = no se pudo usar ese día.

journal.window.speakers.attribution.attribution.response.that.json.request.couldn.t.be = no se pudo leer esa solicitud JSON.

journal.window.speakers.attribution.attribution.response.that.request.didn.t.finish = esa solicitud no terminó.

journal.window.speakers.attribution.attribution.response.that.request.had.no.data.in = esa solicitud no tenía datos.

journal.window.speakers.attribution.attribution.response.that.segment.or.stream.couldn.t = ese segmento o flujo no se pudo usar.

journal.window.speakers.attribution.attribution.response.that.sentence.couldn.t.be.found = no se encontró esa oración. vuelve a cargar la página.

journal.window.speakers.attribution.attribution.response.that.speaker.command.can.t.run = ese comando de hablante no puede ejecutarse hasta que tu voz de propietario esté configurada.

journal.window.speakers.attribution.attribution.response.that.speaker.command.couldn.t.run = ese comando de hablante no se pudo ejecutar porque tu identidad de propietario configurada necesita atención.

journal.window.speakers.attribution.attribution.response.that.speaker.command.didn.t.finish = ese comando de hablante no terminó.

journal.window.speakers.attribution.attribution.response.that.speaker.couldn.t.be.found = no se encontró ese hablante.

journal.window.speakers.attribution.attribution.response.that.speaker.couldn.t.be.used = no se pudo usar ese hablante porque está oculto.

journal.window.speakers.attribution.attribution.response.that.speaker.couldn.t.be.used.variant2 = no se pudo usar ese hablante porque no es una persona.

journal.window.speakers.attribution.attribution.response.that.speaker.review.couldn.t.be = no se pudo cargar esa revisión de hablante.

journal.window.speakers.attribution.attribution.response.that.voice.couldn.t.be.saved = no se pudo guardar esa voz porque suena demasiado a la tuya.

journal.window.speakers.attribution.attribution.response.that.voice.couldn.t.be.saved.variant2 = no se pudo guardar esa voz porque la referencia de tu voz de propietario necesita atención.

journal.window.speakers.attribution.attribution.response.that.voice.couldn.t.be.updated = no se pudo actualizar esa voz ahora porque estaba ocupada. vuelve a intentarlo en un momento.

journal.window.speakers.attribution.attribution.response.those.speaker.attributions.couldn.t.be = no se pudieron actualizar esas atribuciones de hablante ahora porque estaban ocupadas. vuelve a intentarlo en un momento.

journal.window.speakers.button.label.all = todos

journal.window.speakers.button.label.confirm = confirmar ✓

journal.window.speakers.button.label.corrections = correcciones

journal.window.speakers.button.label.load.more.segments = cargar más segmentos

journal.window.speakers.button.label.look.for.new.voices = buscar voces nuevas

journal.window.speakers.button.label.needs.review = necesita revisión

journal.window.speakers.button.label.show.sentences.and.details = mostrar oraciones y detalles

journal.window.speakers.cli.owner.candidate.guidance = Confirma esta candidata si es tu voz.

journal.window.speakers.cli.suggest.candidate.pair = Par de candidatos de hablante: similitud { $similarity } ({ $source_intervals } frente a { $target_intervals } intervalos)

journal.window.speakers.cli.suggest.empty = No se encontraron sugerencias de curaduría de hablantes.

journal.window.speakers.cli.suggest.import.linkable =
    { $meetings_quantity ->
        [one] se puede enlazar: { $name } ({ $entity_id }), se menciona en { $meetings } reunión
              días: { $days }
        [many] se puede enlazar: { $name } ({ $entity_id }), se menciona en { $meetings } reuniones
              días: { $days }
       *[other] se puede enlazar: { $name } ({ $entity_id }), se menciona en { $meetings } reuniones
              días: { $days }
    }

journal.window.speakers.cli.suggest.low.confidence = Revisión de confianza baja: { $day }/{ $segment_key } — { $medium } de { $total } etiquetas son de confianza media o están sin resolver

journal.window.speakers.confidence.accessible =
    { $confidence ->
        [high] confianza alta
       *[medium] confianza media
    }

journal.window.speakers.confidence.label =
    { $confidence ->
        [high] alta
       *[medium] media
    }

journal.window.speakers.copy.action.not.now = ahora no

journal.window.speakers.copy.action.retry = reintentar

journal.window.speakers.copy.action.set.aside = apartar

journal.window.speakers.copy.action.who.is.this = ¿quién es?

journal.window.speakers.copy.anchor = en tu diario

journal.window.speakers.copy.anchor.has.voice = en tu diario · su voz ya está ahí

journal.window.speakers.copy.check.name.error = no se pudo revisar este nombre. vuelve a intentarlo.

journal.window.speakers.copy.correct.propagate.apply = Aplicar estos cambios

journal.window.speakers.copy.correct.propagate.none = No cambiaría nada más.

journal.window.speakers.copy.correct.propagate.offer = se puede buscar a estos dos en los segmentos para mostrarte qué más cambiaría.

journal.window.speakers.copy.correct.scope = Esto corrige esta única frase.

journal.window.speakers.copy.discovery.error = no se pudo buscar voces nuevas ahora.

journal.window.speakers.copy.exit.not.now = ahora no

journal.window.speakers.copy.exit.not.person = no es una persona

journal.window.speakers.copy.filter.by.prefix = filtrando por:

journal.window.speakers.copy.filter.no.segments.today = no hay segmentos atribuidos a este hablante hoy

journal.window.speakers.copy.grid.activity.one = segmento, todos con nombre

journal.window.speakers.copy.grid.activity.other = segmentos, todos con nombre

journal.window.speakers.copy.grid.body = días con segmentos que todavía necesitan nombres

journal.window.speakers.copy.grid.title = voces por nombrar

journal.window.speakers.copy.grid.unit.none = día tranquilo

journal.window.speakers.copy.grid.unit.one = segmento por nombrar

journal.window.speakers.copy.grid.unit.other = segmentos por nombrar

journal.window.speakers.copy.keep.separate.confirm = sí, alguien nuevo

journal.window.speakers.copy.load.error = no se pudo cargar esta voz. vuelve a intentarlo.

journal.window.speakers.copy.near.match.band = ¿ya está en tu diario?

journal.window.speakers.copy.not.now.done = no hay apuro. se queda esperando en silencio en voces nuevas.

journal.window.speakers.copy.not.person.done = entendido, no es una persona. esta se deja aparte y no se vuelve a preguntar por ella.

journal.window.speakers.copy.overview.card.last.heard.prefix = última vez en tu diario

journal.window.speakers.copy.overview.card.samples.label = muestras

journal.window.speakers.copy.overview.card.segments.label = segmentos

journal.window.speakers.copy.overview.card.streams.prefix = flujos

journal.window.speakers.copy.overview.cohesion.labels.item1 = aprendiendo

journal.window.speakers.copy.overview.cohesion.labels.item2 = inicial

journal.window.speakers.copy.overview.cohesion.labels.item3 = mejorando

journal.window.speakers.copy.overview.cohesion.labels.item4 = buena

journal.window.speakers.copy.overview.cohesion.labels.item5 = fuerte

journal.window.speakers.copy.overview.cohesion.labels.item6 = estable

journal.window.speakers.copy.overview.cohesion.unmeasured = todavía sin medir

journal.window.speakers.copy.overview.known.voices.empty = todavía no se ha aprendido la voz de nadie más. cuando la misma voz aparezca en unos cuantos segmentos, esa persona va a aparecer aquí. también puedes etiquetar voces a mano en cualquier segmento.

journal.window.speakers.copy.overview.known.voices.header = voces conocidas

journal.window.speakers.copy.overview.known.voices.quality.hint = la palabra junto a cada voz muestra con qué confianza se la reconoce, no cuánta de esa voz hay.

journal.window.speakers.copy.overview.known.voices.sorts.item1 = reciente

journal.window.speakers.copy.overview.known.voices.sorts.item2 = más muestras

journal.window.speakers.copy.overview.known.voices.sorts.item3 = alfabético

journal.window.speakers.copy.overview.new.voices.header = voces nuevas

journal.window.speakers.copy.overview.owner.build.from.tags.label = guardar mi voz

journal.window.speakers.copy.overview.owner.built.prefix = construida

journal.window.speakers.copy.overview.owner.built.unknown = desconocido

journal.window.speakers.copy.overview.owner.cohesion.label = consistencia

journal.window.speakers.copy.overview.owner.help.label = enseñar mi voz

journal.window.speakers.copy.overview.owner.progress.unknown = avance del aprendizaje no disponible

journal.window.speakers.copy.overview.owner.refreshed.prefix = actualizada

journal.window.speakers.copy.overview.owner.samples.cap.suffix = (el límite de muestras)

journal.window.speakers.copy.overview.owner.samples.label = muestras de voz

journal.window.speakers.copy.overview.owner.status.error = no se pudo cargar el estado de la voz

journal.window.speakers.copy.overview.owner.streams.label = dónde aparece esta voz

journal.window.speakers.copy.overview.owner.streams.unknown = todavía sin determinar

journal.window.speakers.copy.overview.quality.error.heading = no se pudo cargar la calidad de la voz

journal.window.speakers.copy.overview.quality.header = calidad de la voz

journal.window.speakers.copy.overview.quality.high.label = confianza alta

journal.window.speakers.copy.overview.quality.margin.label = coincidencias cercanas de voz

journal.window.speakers.copy.overview.quality.medium.label = necesita un vistazo

journal.window.speakers.copy.overview.quality.missing.label = segmentos sin procesar

journal.window.speakers.copy.overview.quality.prebootstrap = tu voz todavía está aprendiendo

journal.window.speakers.copy.overview.quality.skipped.label = aparte por ahora

journal.window.speakers.copy.overview.quality.teaching.zero = no hay cambios de enseñanza recientes

journal.window.speakers.copy.overview.quality.unlabeled.label = todavía sin nombre

journal.window.speakers.copy.overview.quality.unreadable.warning = no se pudieron leer algunos archivos locales de hablantes

journal.window.speakers.copy.overview.today.link.label = revisión de hoy →

journal.window.speakers.copy.overview.your.voice.confirmed = confirmada

journal.window.speakers.copy.overview.your.voice.header = tu voz

journal.window.speakers.copy.overview.your.voice.learning = todavía sin confirmar

journal.window.speakers.copy.owner.build.failed = no se pudo guardar tu voz. vuelve a intentarlo en un momento.

journal.window.speakers.copy.owner.candidate.effect = si eres tú, las conversaciones pasadas muestran tus líneas como "tú". en las actividades de hoy y de los últimos 7 días se reescriben sus personas y su relato para que coincidan, y las más antiguas se quedan como se escribieron.

journal.window.speakers.copy.owner.candidate.title = ¿es tu voz?

journal.window.speakers.copy.owner.confirm.label = sí, soy yo

journal.window.speakers.copy.owner.identity.needed = agrega tu nombre en la configuración para que tu diario pueda distinguir cuál voz es la tuya.

journal.window.speakers.copy.owner.identity.needed.label = agregar mi nombre

journal.window.speakers.copy.owner.not.ready.needs.more = tu diario busca tu voz cada día y te pregunta cuando encuentra una. también puedes enseñarla ahora.

journal.window.speakers.copy.owner.not.ready.short.or.mixed = hasta ahora tus frases son sobre todo cortas, o están mezcladas con otras voces, así que enseñar tu voz es lo que más ayuda.

journal.window.speakers.copy.owner.redo.body = marca las frases que son tuyas y después actualiza tu voz. si no coinciden lo bastante con tu voz guardada, no cambia nada.

journal.window.speakers.copy.owner.redo.done = tu voz está actualizada

journal.window.speakers.copy.owner.redo.label = enseñar mi voz otra vez

journal.window.speakers.copy.owner.redo.not.enough = enseña unas cuantas frases más largas y después actualiza tu voz.

journal.window.speakers.copy.owner.redo.refused = lo que enseñaste no coincide lo bastante con tu voz guardada, así que tu diario conservó la guardada. si tu voz cambió, quita la guardada en la página de hablantes y enséñala otra vez.

journal.window.speakers.copy.owner.redo.save.label = actualizar mi voz

journal.window.speakers.copy.owner.redo.unchanged = tu voz ya coincide con lo que enseñaste, así que no cambió nada.

journal.window.speakers.copy.owner.reject.label = no, no soy yo

journal.window.speakers.copy.owner.relabel.done = las conversaciones pasadas ahora muestran qué líneas son tuyas.

journal.window.speakers.copy.owner.relabel.scope = en las actividades de hoy y de los últimos 7 días se reescriben sus personas y su relato para que coincidan, y las más antiguas se quedan como se escribieron. las conversaciones nuevas muestran tus líneas como "tú" de ahora en adelante.

journal.window.speakers.copy.owner.relabel.started = se están actualizando las conversaciones pasadas para mostrar qué líneas son tuyas.

journal.window.speakers.copy.owner.relabel.waiting = las conversaciones pasadas se van a actualizar dentro de un día.

journal.window.speakers.copy.owner.reveal.close.label = continuar

journal.window.speakers.copy.owner.reveal.title = tu voz está confirmada

journal.window.speakers.copy.owner.teach.body = elige frases más largas que sean tuyas. mantienen tu voz separada de otros hablantes.

journal.window.speakers.copy.owner.teach.busy = la actualización de la voz está ocupada; intenta con la siguiente frase en un momento

journal.window.speakers.copy.owner.teach.empty = todavía no hay nada de las últimas dos semanas listo para enseñar

journal.window.speakers.copy.owner.teach.exhausted.title = eso es todo por ahora

journal.window.speakers.copy.owner.teach.failed = la actualización de la voz no terminó; intenta con otra frase en un momento

journal.window.speakers.copy.owner.teach.instead.label = enseñar mi voz en su lugar

journal.window.speakers.copy.owner.teach.loading = buscando frases que se puedan enseñar...

journal.window.speakers.copy.owner.teach.mine.label = soy yo

journal.window.speakers.copy.owner.teach.not.me.label = no soy yo

journal.window.speakers.copy.owner.teach.not.me.response = ok, sigo

journal.window.speakers.copy.owner.teach.partial = no se pudieron incluir algunas grabaciones recientes

journal.window.speakers.copy.owner.teach.pause.label = pausar

journal.window.speakers.copy.owner.teach.paused = enseñanza en pausa

journal.window.speakers.copy.owner.teach.refused.title = todavía no está listo

journal.window.speakers.copy.owner.teach.skip.label = omitir por ahora

journal.window.speakers.copy.owner.teach.start.label = empezar a enseñar

journal.window.speakers.copy.owner.teach.title = enseñar tu voz

journal.window.speakers.copy.owner.undo.cancel.label = conservarla

journal.window.speakers.copy.owner.undo.confirm.body = las conversaciones pasadas vuelven a voces sin nombre. tu diario vuelve a buscar tu voz, pero no va a ofrecer esta. las etiquetas que pusiste tú y las personas que nombraste se quedan como están. en las actividades de hoy y de los últimos 7 días se reescriben sus personas y su relato para que coincidan, y las más antiguas se quedan como se escribieron.

journal.window.speakers.copy.owner.undo.confirm.label = quitar mi voz

journal.window.speakers.copy.owner.undo.confirm.title = ¿quitar tu voz guardada?

journal.window.speakers.copy.owner.undo.done = las conversaciones pasadas volvieron a voces sin nombre. las etiquetas que pusiste tú y las personas que nombraste se quedaron.

journal.window.speakers.copy.owner.undo.failed = no se pudo quitar tu voz. vuelve a intentarlo en un momento.

journal.window.speakers.copy.owner.undo.label = quitar mi voz

journal.window.speakers.copy.play.from.here.label = reproducir desde aquí

journal.window.speakers.copy.preview.back = atrás

journal.window.speakers.copy.preview.body.fresh = en todos los lugares donde esta voz ha aparecido, en cada conversación, ahora y de aquí en adelante.

journal.window.speakers.copy.receipt.undo = deshacer

journal.window.speakers.copy.sample.unavailable = esta muestra no está disponible

journal.window.speakers.copy.sample.unverified.evidence = evidencia sin verificar

journal.window.speakers.copy.search.error = no se pudo buscar en tus personas. vuelve a intentarlo.

journal.window.speakers.copy.search.label = otra persona

journal.window.speakers.copy.search.placeholder = buscar en tus personas

journal.window.speakers.copy.sheet.title = alguien sigue apareciendo

journal.window.speakers.copy.shelf.candidates = estas personas estaban cerca de esta voz:

journal.window.speakers.copy.shelf.mentions = también cerca, de nombres que aparecen alrededor de esta voz:

journal.window.speakers.copy.shelf.no.evidence = no había nadie más cerca de esta voz. busca en tus personas:

journal.window.speakers.copy.this.is.me = soy yo

journal.window.speakers.copy.this.is.me.guidance = tu voz se enseña por separado. enséñala en la configuración de tu voz.

journal.window.speakers.copy.tr.not.in.new.voices = esta voz todavía no está en voces nuevas. tiene que aparecer algunas veces más.

journal.window.speakers.copy.undo.done = deshecho. esta voz volvió a voces nuevas.

journal.window.speakers.copy.view.voice.samples = ver muestras de voz

journal.window.speakers.day.discovery.pattern.count =
    { $count_quantity ->
        [one] { $count } patrón de voz recurrente no coincide con nadie que hayas nombrado. escucha algunas muestras y ponles nombre.
        [many] { $count } patrones de voz recurrentes no coinciden con nadie que hayas nombrado. escucha algunas muestras y ponles nombre.
       *[other] { $count } patrones de voz recurrentes no coinciden con nadie que hayas nombrado. escucha algunas muestras y ponles nombre.
    }

journal.window.speakers.day.discovery.unnamed.count =
    { $count_quantity ->
        [one] voces sin nombre en todo tu diario ({ $count })
        [many] voces sin nombre en todo tu diario ({ $count })
       *[other] voces sin nombre en todo tu diario ({ $count })
    }

journal.window.speakers.disclosure.label.why.not.yet = ¿por qué todavía no?

journal.window.speakers.discovery.invalid.embeddings = se omitieron algunas muestras de voz porque no se podían usar.

journal.window.speakers.discovery.labels.busy = no se pudieron actualizar las etiquetas de hablante porque hay otra actualización en curso.

journal.window.speakers.discovery.not.ran = todavía no se han buscado voces nuevas. usa "buscar voces nuevas" arriba para empezar.

journal.window.speakers.discovery.owner.identity.invalid = no se pudo empezar la búsqueda de voces nuevas porque la identidad de propietario configurada necesita atención.

journal.window.speakers.discovery.owner.voice.unavailable = tu voz tiene que configurarse antes de que pueda empezar la búsqueda de voces nuevas.

journal.window.speakers.discovery.sample.count =
    { $count_quantity ->
        [one] { $count } muestra
        [many] { $count } muestras
       *[other] { $count } muestras
    }

journal.window.speakers.discovery.segment.count =
    { $count_quantity ->
        [one] { $count } segmento
        [many] { $count } segmentos
       *[other] { $count } segmentos
    }

journal.window.speakers.discovery.unlabeled.count =
    { $count_quantity ->
        [one] voces todavía sin nombre ({ $count })
        [many] voces todavía sin nombre ({ $count })
       *[other] voces todavía sin nombre ({ $count })
    }

journal.window.speakers.discovery.unused.samples =
    { $samples_quantity ->
        [one] no se pudo usar una muestra de voz ({ $samples })
        [many] no se pudieron usar algunas muestras de voz ({ $samples })
       *[other] no se pudieron usar algunas muestras de voz ({ $samples })
    }

journal.window.speakers.discovery.voice.changed = esa voz cambió desde que se cargó esta página. vuelve a cargar la página.

journal.window.speakers.discovery.voice.not.found = no se encontró esa voz. vuelve a cargar la página.

journal.window.speakers.discovery.voice.owner.unavailable = tu propia voz tiene que configurarse primero, para que no quede nombrada como otra persona.

journal.window.speakers.discovery.voice.principal = tu propia voz se configura por separado, no aquí.

journal.window.speakers.discovery.voiceprint.busy = no se pudo actualizar esa voz porque hay otra actualización en curso.

journal.window.speakers.error.attribution.owner.identity.invalid = no se pudo cambiar esa atribución de hablante porque la identidad de propietario configurada necesita atención.

journal.window.speakers.error.attribution.state.invalid = no se pudo cambiar esa atribución de hablante.

journal.window.speakers.error.change.conflict = ese cambio de hablante no puede ejecutarse ahora.

journal.window.speakers.error.change.not.found = no se encontró ese cambio de hablante.

journal.window.speakers.error.change.one.way = nombrar una voz no se puede deshacer.

journal.window.speakers.error.change.recoverable = ese cambio de hablante no terminó. vuelve a intentarlo en un momento.

journal.window.speakers.error.change.repair.required = ese cambio de hablante se detuvo a la mitad y todavía no puede terminar.

journal.window.speakers.error.configured.owner.identity.is.not.admitted = la identidad de propietario configurada no está admitida

journal.window.speakers.error.embedding.too.similar.to.owner.voice.cannot.save = Incrustación demasiado parecida a la voz del propietario; no se puede guardar

journal.window.speakers.error.files.busy = los archivos de hablante están ocupados. vuelve a intentarlo en un momento.

journal.window.speakers.error.invalid.day.format = Formato de día no válido

journal.window.speakers.error.invalid.limit.offset.parameter = Parámetro limit/offset no válido

journal.window.speakers.error.invalid.month.format.expected.yyyymm = Formato de mes no válido, se espera YYYYMM

journal.window.speakers.error.invalid.sort.parameter = Parámetro de orden no válido

journal.window.speakers.error.invalid.speaker.parameter = Parámetro de hablante no válido

journal.window.speakers.error.no.transcript.found = No se encontró una transcripción

journal.window.speakers.error.one.of.those.values.couldn.t.be.used = no se pudo usar uno de esos valores.

journal.window.speakers.error.owner.confirm.identity.invalid = no se pudo confirmar esa voz de propietario porque la identidad de propietario configurada necesita atención.

journal.window.speakers.error.owner.confirm.review.required = Confirma esta candidata en la página de hablantes del navegador.

journal.window.speakers.error.sentence.not.found = no se encontró esa oración.

journal.window.speakers.error.speaker.quality.couldn.t.be.loaded.because.your.configured.owner.identity = no se pudo cargar la calidad del hablante porque la identidad de propietario configurada necesita atención.

journal.window.speakers.error.status.owner.identity.invalid = no se pudo cargar el estado del hablante porque la identidad de propietario configurada necesita atención.

journal.window.speakers.error.task.conflict = esa tarea de hablante ya está en ejecución con opciones distintas.

journal.window.speakers.error.task.not.found = no se encontró esa tarea de hablante.

journal.window.speakers.error.that.day.couldn.t.be.used = no se pudo usar ese día.

journal.window.speakers.error.that.month.couldn.t.be.used = no se pudo usar ese mes.

journal.window.speakers.error.that.request.couldn.t.be.used = esa solicitud no se pudo usar.

journal.window.speakers.error.that.request.didn.t.finish = esa solicitud no terminó.

journal.window.speakers.error.that.speaker.command.didn.t.finish = ese comando de hablante no terminó.

journal.window.speakers.error.that.speaker.review.couldn.t.be.loaded = no se pudo cargar esa revisión de hablante.

journal.window.speakers.error.voice.sentence.id.must.be.an.integer = voice_sentence_id tiene que ser un entero

journal.window.speakers.error.wipe.busy = tu diario está ocupado con grabaciones ahora; vuelve a intentarlo en un momento.

journal.window.speakers.error.wipe.held = tus voces no se pudieron borrar ahora, así que no se borró nada.

journal.window.speakers.error.your.owner.voice.couldn.t.be.loaded.because.your.configured.owner.identity = no se pudo cargar tu voz de propietario porque la identidad de propietario configurada necesita atención.

journal.window.speakers.field.placeholder.find.a.voice.by.name = buscar una voz por nombre

journal.window.speakers.grid.all.named.count =
    { $quantity ->
        [one] 1 segmento, con nombre
        [many] { $count } segmentos, todos con nombre
       *[other] { $count } segmentos, todos con nombre
    }

journal.window.speakers.grid.to.name.count =
    { $quantity ->
        [one] 1 segmento por nombrar
        [many] { $count } segmentos por nombrar
       *[other] { $count } segmentos por nombrar
    }

journal.window.speakers.heading.couldn.t.load.review = no se pudo cargar la revisión

journal.window.speakers.heading.couldn.t.load.segments = no se pudieron cargar los segmentos

journal.window.speakers.heading.couldn.t.load.speakers = no se pudieron cargar los hablantes

journal.window.speakers.heading.no.audio.sources = no hay fuentes de audio

journal.window.speakers.heading.no.sentences.to.review = no hay oraciones para revisar

journal.window.speakers.heading.no.speakers.found = no se encontraron hablantes

journal.window.speakers.known.count.sample.one = { $count }{ $slot1 }muestra

journal.window.speakers.known.count.sample.other = { $count }{ $slot1 }muestras

journal.window.speakers.known.count.segment.one = { $count }{ $slot1 }segmento

journal.window.speakers.known.count.segment.other = { $count }{ $slot1 }segmentos

journal.window.speakers.known.placeholder.count =
    { $count_quantity ->
        [one] voces que todavía usan un nombre provisional ({ $count })
        [many] voces que todavía usan un nombre provisional ({ $count })
       *[other] voces que todavía usan un nombre provisional ({ $count })
    }

journal.window.speakers.known.search.empty = ninguna voz conocida coincide con "{ $query }"

journal.window.speakers.known.show.all.count =
    { $count_quantity ->
        [one] mostrar { $count } voz
        [many] mostrar todas las { $count } voces
       *[other] mostrar todas las { $count } voces
    }

journal.window.speakers.media.error.file.not.found = ese archivo no está disponible.

journal.window.speakers.media.error.invalid.path = no se pudo usar esa ruta.

journal.window.speakers.notice.couldn.t.load.voice.samples = no se pudieron cargar las muestras de voz.

journal.window.speakers.option.label.assign = asignar…

journal.window.speakers.option.label.correct = corregir…

journal.window.speakers.owner.cooldown.until.day = esa voz no se volverá a ofrecer. tu diario vuelve a buscar tu voz después de { $retry_day }, o puedes enseñarle ahora.

journal.window.speakers.owner.line.labels.error.entity.store.unreadable = no se pueden leer las entidades del diario: { $detail }

journal.window.speakers.owner.line.labels.error.file.unreadable = no se pueden leer las etiquetas de línea de propietario del segmento: { $detail }

journal.window.speakers.owner.line.labels.error.line.id.mismatch = la línea { $sentence_id } de { $audio_source } está guardada con el id de oración { $stored }, no con su número de línea

journal.window.speakers.owner.line.labels.error.line.not.found = la línea { $sentence_id } no es una línea de { $audio_source }

journal.window.speakers.owner.line.labels.error.lock = las etiquetas de línea de propietario están ocupadas: { $detail }

journal.window.speakers.owner.line.labels.error.merge.recovery.pending = una fusión de entidades interrumpida está en espera de recuperación

journal.window.speakers.owner.line.labels.error.no.lines = no se nombró ninguna línea

journal.window.speakers.owner.line.labels.error.not.a.person = { $id } no es una persona

journal.window.speakers.owner.line.labels.error.owner.not.admitted = la identidad de propietario del diario no está establecida

journal.window.speakers.owner.line.labels.error.person.hidden = la persona { $id } está oculta

journal.window.speakers.owner.line.labels.error.person.merged = { $id } se fusionó en { $successor }

journal.window.speakers.owner.line.labels.error.person.not.found = no se encontró a la persona { $id }

journal.window.speakers.owner.line.labels.error.segment.unavailable = el segmento no admite etiquetas de línea de propietario: { $detail }

journal.window.speakers.owner.line.labels.error.source.unsupported = las vistas de transcripción del segmento no coinciden en una sola fuente de audio (etiquetas: { $labelled }, página de transcripción: { $transcript_page })

journal.window.speakers.owner.line.labels.error.write = no se pudieron escribir las etiquetas de línea de propietario: { $detail }

journal.window.speakers.owner.line.labels.error.wrong.source = las líneas de { $requested } no se pueden etiquetar; la fuente etiquetada del segmento es { $labelled }

journal.window.speakers.owner.recordings.audio.count =
    { $recordings_quantity ->
        [one] { $recordings } grabación con audio hasta ahora
        [many] { $recordings } grabaciones con audio hasta ahora
       *[other] { $recordings } grabaciones con audio hasta ahora
    }

journal.window.speakers.owner.relabel.incomplete =
    { $count_quantity ->
        [one] las conversaciones pasadas están actualizadas, excepto { $count } grabación que no se pudo actualizar.
        [many] las conversaciones pasadas están actualizadas, excepto { $count } grabaciones que no se pudieron actualizar.
       *[other] las conversaciones pasadas están actualizadas, excepto { $count } grabaciones que no se pudieron actualizar.
    }

journal.window.speakers.owner.relabel.progress =
    { $completed_quantity ->
        [one] actualizando conversaciones pasadas: { $completed } de { $total_quantity ->
        [one] { $total } grabación
        [many] { $total } grabaciones
       *[other] { $total } grabaciones
    }
        [many] actualizando conversaciones pasadas: { $completed } de { $total_quantity ->
        [one] { $total } grabación
        [many] { $total } grabaciones
       *[other] { $total } grabaciones
    }
       *[other] actualizando conversaciones pasadas: { $completed } de { $total_quantity ->
        [one] { $total } grabación
        [many] { $total } grabaciones
       *[other] { $total } grabaciones
    }
    }

journal.window.speakers.owner.relabel.undo.progress =
    { $completed_quantity ->
        [one] devolviendo conversaciones pasadas a voces sin nombre: { $completed } de { $total_quantity ->
        [one] { $total } grabación
        [many] { $total } grabaciones
       *[other] { $total } grabaciones
    }
        [many] devolviendo conversaciones pasadas a voces sin nombre: { $completed } de { $total_quantity ->
        [one] { $total } grabación
        [many] { $total } grabaciones
       *[other] { $total } grabaciones
    }
       *[other] devolviendo conversaciones pasadas a voces sin nombre: { $completed } de { $total_quantity ->
        [one] { $total } grabación
        [many] { $total } grabaciones
       *[other] { $total } grabaciones
    }
    }

journal.window.speakers.owner.reveal.evidence.tier = { $evidence_tier } evidencia

journal.window.speakers.owner.reveal.stream.count =
    { $streams_quantity ->
        [one] aparece en 1 lugar
        [many] aparece en { $streams } lugares
       *[other] aparece en { $streams } lugares
    }

journal.window.speakers.owner.status.error = no se pudo cargar el estado del propietario: { $detail }

journal.window.speakers.owner.teaching.exhausted.progress =
    { $statements_quantity ->
        [one] { $statements } de { $required_quantity ->
        [one] { $required } enunciado más largo
        [many] { $required } enunciados más largos
       *[other] { $required } enunciados más largos
    } enseñado
        [many] { $statements } de { $required_quantity ->
        [one] { $required } enunciado más largo
        [many] { $required } enunciados más largos
       *[other] { $required } enunciados más largos
    } enseñados
       *[other] { $statements } de { $required_quantity ->
        [one] { $required } enunciado más largo
        [many] { $required } enunciados más largos
       *[other] { $required } enunciados más largos
    } enseñados
    }

journal.window.speakers.owner.teaching.progress =
    { $statements_quantity ->
        [one] { $statements } de { $required_quantity ->
        [one] { $required } enunciado más largo
        [many] { $required } enunciados más largos
       *[other] { $required } enunciados más largos
    }
        [many] { $statements } de { $required_quantity ->
        [one] { $required } enunciado más largo
        [many] { $required } enunciados más largos
       *[other] { $required } enunciados más largos
    }
       *[other] { $statements } de { $required_quantity ->
        [one] { $required } enunciado más largo
        [many] { $required } enunciados más largos
       *[other] { $required } enunciados más largos
    }
    }

journal.window.speakers.owner.teaching.statements =
    { $statements_quantity ->
        [one] { $statements } enunciado enseñado
        [many] { $statements } enunciados enseñados
       *[other] { $statements } enunciados enseñados
    }

journal.window.speakers.owner.voice.sample.count =
    { $statements_quantity ->
        [one] esta voz está en { $statements } enunciado de tu diario. reproduce una muestra antes de decidir.
        [many] esta voz está en { $statements } enunciados de tu diario. reproduce algunas muestras antes de decidir.
       *[other] esta voz está en { $statements } enunciados de tu diario. reproduce algunas muestras antes de decidir.
    }

journal.window.speakers.owner.write.owner.voice.error.detail.audio.sha256.must.be.a.non = audio_sha256 tiene que ser un hash hexadecimal no vacío

journal.window.speakers.owner.write.owner.voice.error.detail.awareness.state.is.not.candidate = El estado de awareness no es candidato

journal.window.speakers.owner.write.owner.voice.error.detail.candidate.version.mismatch.or.stale.generation = La versión del candidato no coincide, o la generación está desactualizada

journal.window.speakers.owner.write.owner.voice.error.detail.configured.owner.identity.is.not.admitted = la identidad de propietario configurada no está admitida

journal.window.speakers.owner.write.owner.voice.error.detail.invalid.day.format = Formato de día no válido

journal.window.speakers.owner.write.owner.voice.error.detail.invalid.segment.key = La clave del segmento no es válida

journal.window.speakers.owner.write.owner.voice.error.detail.missing.required.fields = Faltan campos obligatorios

journal.window.speakers.owner.write.owner.voice.error.detail.no.candidate.available = No hay un candidato disponible

journal.window.speakers.owner.write.owner.voice.error.detail.no.data.provided = No se proporcionaron datos

journal.window.speakers.owner.write.owner.voice.error.detail.no.owner.voice.is.saved = No hay una voz de propietario guardada

journal.window.speakers.owner.write.owner.voice.error.detail.owner.build.failed = no se pudo construir la voz de propietario

journal.window.speakers.owner.write.owner.voice.error.detail.request.body.must.be.a.json = el cuerpo de la solicitud debe ser un objeto JSON

journal.window.speakers.owner.write.owner.voice.error.detail.sample.hash.verification.failed = No se pudo verificar el hash de la muestra

journal.window.speakers.owner.write.owner.voice.error.detail.sample.not.found.in.active.candidate = No se encontró la muestra en las muestras del candidato activo

journal.window.speakers.owner.write.owner.voice.error.detail.use.one.file.name.without.a = usa un solo nombre de archivo, sin ruta

journal.window.speakers.owner.write.owner.voice.guidance.owner.centroid.already.exists.run.solstone = El centroide de propietario ya existe. Ejecuta { $slot1 } para actualizarlo desde las etiquetas manuales actuales.

journal.window.speakers.owner.write.owner.voice.response.a.required.field.is.missing = falta un campo obligatorio.

journal.window.speakers.owner.write.owner.voice.response.source.name.is.invalid = el nombre de la fuente no es válido.

journal.window.speakers.owner.write.owner.voice.response.that.day.couldn.t.be.used = no se pudo usar ese día.

journal.window.speakers.owner.write.owner.voice.response.that.json.request.couldn.t.be = no se pudo leer esa solicitud JSON.

journal.window.speakers.owner.write.owner.voice.response.that.person.couldn.t.be.found = no se pudo encontrar a esa persona.

journal.window.speakers.owner.write.owner.voice.response.that.request.had.no.data.in = esa solicitud no tenía datos.

journal.window.speakers.owner.write.owner.voice.response.that.segment.or.stream.couldn.t = ese segmento o flujo no se pudo usar.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.candidate.is.no.longer = ese candidato de hablante ya no es el actual.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.command.couldn.t.run = ese comando de hablante no se pudo ejecutar porque tu identidad de propietario configurada necesita atención.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.command.didn.t.finish = ese comando de hablante no terminó.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.review.couldn.t.be = no se pudo cargar esa revisión de hablante.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.sample.could.not.be = no se pudo verificar esa muestra de hablante contra la evidencia en disco.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.sample.does.not.have = esa muestra de hablante no tiene hashes de evidencia válidos.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.sample.has.unverified.audio = esa muestra de hablante tiene evidencia de audio sin verificar.

journal.window.speakers.owner.write.owner.voice.response.that.speaker.sample.is.not.part = esa muestra de hablante no es parte del candidato activo.

journal.window.speakers.owner.write.owner.voice.response.that.voice.couldn.t.be.updated = no se pudo actualizar esa voz ahora porque estaba ocupada. vuelve a intentarlo en un momento.

journal.window.speakers.owner.write.owner.voice.response.there.s.no.saved.voice.to = no hay una voz guardada para deshacer.

journal.window.speakers.quality.buckets.empty = nada ahora: { $empty_categories }

journal.window.speakers.quality.progress.statements =
    { $count_quantity ->
        [one] { $count } enunciado más largo
        [many] { $count } enunciados más largos
       *[other] { $count } enunciados más largos
    }

journal.window.speakers.quality.statement.denominator =
    { $count_quantity ->
        [one] a lo largo de { $count } enunciado
        [many] a lo largo de { $count } enunciados
       *[other] a lo largo de { $count } enunciados
    }

journal.window.speakers.quality.teaching.count =
    { $count_quantity ->
        [one] { $count } cambio de enseñanza
        [many] { $count } cambios de enseñanza
       *[other] { $count } cambios de enseñanza
    }

journal.window.speakers.quality.window.days =
    { $days_quantity ->
        [one] etiquetas de hablante, último { $days } día
        [many] etiquetas de hablante, últimos { $days } días
       *[other] etiquetas de hablante, últimos { $days } días
    }

journal.window.speakers.sample.ordinal = muestra { $sample_number }

journal.window.speakers.section.heading.ready.to.review = listo para revisar

journal.window.speakers.segment.attributed.total =
    { $attributed_quantity ->
        [one] { $attributed }/{ $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } atribuido
        [many] { $attributed }/{ $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } atribuidos
       *[other] { $attributed }/{ $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } atribuidos
    }

journal.window.speakers.segment.meeting.matching.prompt =
    { $count_quantity ->
        [one] esta reunión tiene { $count } hablante con nombre, pero su voz todavía no se puede emparejar. asignar aunque sea unas pocas oraciones mejorará el emparejamiento de voces para reuniones futuras.
        [many] esta reunión tiene { $count } hablantes con nombre, pero sus voces todavía no se pueden emparejar. asignar aunque sea unas pocas oraciones mejorará el emparejamiento de voces para reuniones futuras.
       *[other] esta reunión tiene { $count } hablantes con nombre, pero sus voces todavía no se pueden emparejar. asignar aunque sea unas pocas oraciones mejorará el emparejamiento de voces para reuniones futuras.
    }

journal.window.speakers.segment.review.badge =
    { $quantity ->
        [one] { $count } para revisar
        [many] { $count } para revisar
       *[other] { $count } para revisar
    }

journal.window.speakers.segment.speaker.count =
    { $count_quantity ->
        [one] { $count } hablante
        [many] { $count } hablantes
       *[other] { $count } hablantes
    }

journal.window.speakers.segment.speakers =
    { $speakers_quantity ->
        [one] { $start_time } - { $end_time }, { $speakers } hablante
        [many] { $start_time } - { $end_time }, { $speakers } hablantes
       *[other] { $start_time } - { $end_time }, { $speakers } hablantes
    }

journal.window.speakers.segment.speakers.and.review =
    { $speakers_quantity ->
        [one] { $start_time } - { $end_time }, { $speakers } hablante, { $review_quantity ->
        [one] { $needs_review } para revisar
        [many] { $needs_review } para revisar
       *[other] { $needs_review } para revisar
    }
        [many] { $start_time } - { $end_time }, { $speakers } hablantes, { $review_quantity ->
        [one] { $needs_review } para revisar
        [many] { $needs_review } para revisar
       *[other] { $needs_review } para revisar
    }
       *[other] { $start_time } - { $end_time }, { $speakers } hablantes, { $review_quantity ->
        [one] { $needs_review } para revisar
        [many] { $needs_review } para revisar
       *[other] { $needs_review } para revisar
    }
    }

journal.window.speakers.segment.visible.total =
    { $visible_quantity ->
        [one] { $visible } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
        [many] { $visible } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
       *[other] { $visible } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.speakers.segments.show.more =
    { $shown_quantity ->
        [one] cargar más segmentos ({ $shown } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    })
        [many] cargar más segmentos ({ $shown } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    })
       *[other] cargar más segmentos ({ $shown } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    })
    }

journal.window.speakers.sentences.visible.range =
    { $shown_quantity ->
        [one] oraciones ({ $shown } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    })
        [many] oraciones ({ $shown } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    })
       *[other] oraciones ({ $shown } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    })
    }

journal.window.speakers.status.text.no.name.yet = todavía no hay nombre

journal.window.speakers.status.text.no.transcript = (sin transcripción)

journal.window.speakers.status.text.not.yet.named = todavía sin nombre

journal.window.speakers.status.text.play.from.here = reproducir desde aquí

journal.window.speakers.status.text.set.aside = apartar

journal.window.speakers.status.text.this.sample.isn.t.available = esta muestra no está disponible

journal.window.speakers.status.text.try.again = reintentar

journal.window.speakers.status.text.unverified.evidence = evidencia sin verificar

journal.window.speakers.text.loading.known.voices = cargando voces conocidas…

journal.window.speakers.text.loading.voice.quality = cargando la calidad de voz…

journal.window.speakers.text.loading.your.voice = cargando tu voz…

journal.window.speakers.tooltip.back.s = atrás 5 s

journal.window.speakers.tooltip.playback.speed = velocidad de reproducción

journal.window.speakers.visible.text.couldn.t.load.audio.the.file = no se pudo cargar el audio. el archivo puede faltar o tener un formato que no se admite

journal.window.speakers.visible.text.couldn.t.load.this.audio = no se pudo cargar este audio

journal.window.speakers.visible.text.counted.in.segments = se cuentan en segmentos

journal.window.speakers.visible.text.failed.to.load.speakers = No se pudieron cargar los hablantes

journal.window.speakers.visible.text.loading = cargando…

journal.window.speakers.visible.text.loading.new.voices = cargando voces nuevas…

journal.window.speakers.visible.text.loading.speakers = cargando hablantes…

journal.window.speakers.visible.text.needs.review = necesita revisión

journal.window.speakers.visible.text.no.audio.file.available = no hay un archivo de audio disponible

journal.window.speakers.visible.text.no.sentences.match.this.filter = ninguna oración coincide con este filtro

journal.window.speakers.visible.text.not.yet.identified = todavía sin identificar

journal.window.speakers.visible.text.owner.voiceprint = Huella de voz del propietario

journal.window.speakers.visible.text.select.a.segment.from.the.list = elige un segmento de la lista para revisar las asignaciones de hablante

journal.window.speakers.visible.text.speakers.haven.t.been.identified.yet = los hablantes todavía no están identificados. asigna algunas oraciones para mejorar el emparejamiento de voces.

journal.window.speakers.visible.text.unassigned = sin asignar

journal.window.speakers.visible.text.unknown.speaker = Hablante desconocido

journal.window.speakers.voice.analysis.error = no se pudieron analizar los patrones de voz: { $detail }

journal.window.speakers.who.conversations =
    { $count_quantity ->
        [one] esta voz apareció en 1 conversación, guardada en tu diario. di quién es y se le reconocerá de ahora en adelante.
        [many] esta voz apareció en { $count } conversaciones, todas guardadas en tu diario. di quién es y se le reconocerá de ahora en adelante.
       *[other] esta voz apareció en { $count } conversaciones, todas guardadas en tu diario. di quién es y se le reconocerá de ahora en adelante.
    }

journal.window.speakers.who.create.named.person = agregar "{ $person_name }" como una persona nueva

journal.window.speakers.who.evidence.meeting.notes =
    { $count_quantity ->
        [one] en tus notas de reunión de ese día
        [many] en tus notas de reunión en { $count } de esos días
       *[other] en tus notas de reunión en { $count } de esos días
    }

journal.window.speakers.who.evidence.on.screen =
    { $count_quantity ->
        [one] en pantalla en 1 de estas conversaciones
        [many] en pantalla en { $count } de estas conversaciones
       *[other] en pantalla en { $count } de estas conversaciones
    }

journal.window.speakers.who.identify.confirm.first.name = sí, es { $first_name }

journal.window.speakers.who.identify.preview.existing.voice = { $person_name } ya tiene una voz en tu diario. esto se guardará como otra forma en que suena.

journal.window.speakers.who.identify.preview.heading = esta voz pasa a ser { $person_name }

journal.window.speakers.who.identify.receipt.conversations = todas las conversaciones con esta voz ahora dicen { $person_name }.

journal.window.speakers.who.identify.receipt.heading = la voz de { $person_name } ya está en tu diario

journal.window.speakers.who.keep.separate.decline = no, es { $person_name }

journal.window.speakers.who.keep.separate.heading = ¿diferente de { $person_name }?

journal.window.speakers.who.keep.separate.reason = { $person_name } ya está en tu diario. si es otra persona, se quedará aparte y nunca se mezclará.

journal.window.speakers.who.preview.fact.count =
    { $statements_quantity ->
        [one] { $statements } enunciado en { $conversations_quantity ->
        [one] { $conversations } conversación
        [many] { $conversations } conversaciones
       *[other] { $conversations } conversaciones
    }, todo en tu diario. puedes deshacer esto.
        [many] { $statements } enunciados en { $conversations_quantity ->
        [one] { $conversations } conversación
        [many] { $conversations } conversaciones
       *[other] { $conversations } conversaciones
    }, todo en tu diario. puedes deshacer esto.
       *[other] { $statements } enunciados en { $conversations_quantity ->
        [one] { $conversations } conversación
        [many] { $conversations } conversaciones
       *[other] { $conversations } conversaciones
    }, todo en tu diario. puedes deshacer esto.
    }

journal.window.speakers.who.search.no.person = nadie en tu diario coincide con "{ $query }"

journal.window.speakers.who.undo.partial.count =
    { $restored_quantity ->
        [one] deshacer restauró parte de esta voz. { $restored } volvió atrás; { $skipped_quantity ->
        [one] { $skipped }
        [many] { $skipped }
       *[other] { $skipped }
    } sin cambios. vuelve a intentarlo.
        [many] deshacer restauró parte de esta voz. { $restored } volvieron atrás; { $skipped_quantity ->
        [one] { $skipped }
        [many] { $skipped }
       *[other] { $skipped }
    } sin cambios. vuelve a intentarlo.
       *[other] deshacer restauró parte de esta voz. { $restored } volvieron atrás; { $skipped_quantity ->
        [one] { $skipped }
        [many] { $skipped }
       *[other] { $skipped }
    } sin cambios. vuelve a intentarlo.
    }

journal.window.speakers.window.explanation.couldn.t.load.known.voices = no se pudieron cargar las voces conocidas

journal.window.speakers.window.explanation.couldn.t.load.more.segments = no se pudieron cargar más segmentos

journal.window.speakers.window.explanation.couldn.t.load.new.voices.right = no se pudieron cargar voces nuevas ahora.

journal.window.speakers.window.explanation.couldn.t.refresh.review.showing.last = no se pudo actualizar la revisión. se muestra el último estado conocido.

journal.window.speakers.window.explanation.couldn.t.reload.segments.showing.the = no se pudieron recargar los segmentos. se muestran los últimos segmentos cargados.

journal.window.speakers.window.explanation.look.for.new.voices = buscar voces nuevas

journal.window.speakers.window.explanation.looking.for.new.voices.waits.until = buscar voces nuevas espera hasta que termines con la voz por la que viniste.

journal.window.speakers.window.explanation.no.new.voices.are.waiting = no hay voces nuevas en espera.

journal.window.speakers.window.explanation.no.new.voices.are.waiting.check = no hay voces nuevas en espera. revisa de nuevo cuando quieras.

journal.window.speakers.window.explanation.no.speaker.segments.found.for.this = todavía no hay segmentos de hablante en este día

journal.window.speakers.window.explanation.no.speaker.voices.were.detected.in = no se detectaron voces de hablante en este segmento

journal.window.speakers.window.explanation.no.speakers.discovered.today = hoy no se descubrieron hablantes

journal.window.speakers.window.explanation.nothing.has.looked.for.new.voices = todavía nada ha buscado voces nuevas.

journal.window.speakers.window.explanation.something.went.wrong.try.again = algo salió mal. vuelve a intentarlo

journal.window.speakers.window.explanation.that.speaker.review.couldn.t.be = no se pudo cargar esa revisión de hablante.

journal.window.speakers.window.explanation.this.segment.doesn.t.have.any = este segmento no tiene audio

journal.window.speakers.window.explanation.this.segment.doesn.t.have.any.variant2 = este segmento todavía no tiene oraciones con las que trabajar

journal.window.speakers.window.state.text.attribution.confirmed = Atribución confirmada

journal.window.speakers.window.state.text.context.match = coincidencia de contexto

journal.window.speakers.window.state.text.just.now = justo ahora

journal.window.speakers.window.state.text.looking.for.new.voices = buscando voces nuevas…

journal.window.speakers.window.state.text.only.speaker = único hablante

journal.window.speakers.window.state.text.setting.match = coincidencia de entorno

journal.window.speakers.window.state.text.voice.match = coincidencia de voz

journal.window.speakers.window.state.text.voice.profile.match = coincidencia del perfil de voz

journal.window.speakers.window.state.text.you.assigned = asignaste

journal.window.speakers.window.state.text.you.confirmed = confirmaste

journal.window.speakers.window.state.text.you.corrected = corregiste

journal.window.speakers.window.state.text.you.identified = identificaste

journal.window.speakers.wipe.error.not.utf8 = no se puede leer un nombre de tu diario: { $path }

journal.window.speakers.wipe.error.path = no se pudo resolver la ruta del artefacto: { $error }

journal.window.speakers.wipe.error.read = no se pudo leer el directorio de artefactos en { $path }: { $source }

journal.window.time.duration.day.count =
    { $quantity ->
        [one] { $count } día
        [many] { $count } días
       *[other] { $count } días
    }

journal.window.time.duration.hour.count =
    { $quantity ->
        [one] { $count } hora
        [many] { $count } horas
       *[other] { $count } horas
    }

journal.window.time.duration.minute.count =
    { $quantity ->
        [one] { $count } minuto
        [many] { $count } minutos
       *[other] { $count } minutos
    }

journal.window.time.duration.month.count =
    { $quantity ->
        [one] { $count } mes
        [many] { $count } meses
       *[other] { $count } meses
    }

journal.window.time.duration.second.count =
    { $quantity ->
        [one] { $count } segundo
        [many] { $count } segundos
       *[other] { $count } segundos
    }

journal.window.time.duration.week.count =
    { $quantity ->
        [one] { $count } semana
        [many] { $count } semanas
       *[other] { $count } semanas
    }

journal.window.transcripts.accessible.label.current.time = hora actual

journal.window.transcripts.accessible.label.screenshot.viewer = visor de capturas de pantalla

journal.window.transcripts.accessible.name.body.context.events = eventos de contexto del cuerpo

journal.window.transcripts.accessible.name.close = cerrar

journal.window.transcripts.accessible.name.copy.stream.id = copiar el id del flujo

journal.window.transcripts.accessible.name.day.timeline = línea de tiempo del día

journal.window.transcripts.accessible.name.items = elementos

journal.window.transcripts.also.at.this.time = también a esta hora:

journal.window.transcripts.also.item = { $label }, { $span }

journal.window.transcripts.analysis.analyzing.audio = audio recibido · analizando…

journal.window.transcripts.analysis.analyzing.image = fotos · analizando…

journal.window.transcripts.analysis.analyzing.other = { $modality } recibido · analizando…

journal.window.transcripts.analysis.analyzing.screen = pantalla recibida · analizando…

journal.window.transcripts.analysis.failed.audio = audio recibido · no se pudo analizar

journal.window.transcripts.analysis.failed.image = fotos · no se pudo analizar

journal.window.transcripts.analysis.failed.other = { $modality } recibido · no se pudo analizar

journal.window.transcripts.analysis.failed.screen = pantalla recibida · no se pudo analizar

journal.window.transcripts.analysis.pending.audio = audio recibido · análisis pendiente

journal.window.transcripts.analysis.pending.image = fotos · análisis pendiente

journal.window.transcripts.analysis.pending.other = { $modality } recibido · análisis pendiente

journal.window.transcripts.analysis.pending.screen = pantalla recibida · análisis pendiente

journal.window.transcripts.analysis.request.failed = no se pudo completar la solicitud de análisis

journal.window.transcripts.analysis.state.analyzing = analizando

journal.window.transcripts.analysis.state.pending = pendiente

journal.window.transcripts.analyzing.label.label.analyzing = analizando…

journal.window.transcripts.audio.warning.capture = el audio puede estar incompleto en este segmento.{ $slot1 }{ $stage }:{ $slot2 }{ $domain }{ $slot3 }({ $code })

journal.window.transcripts.audio.warning.history = un intento anterior de copiar el audio tuvo un problema. la copia más reciente se completó.

journal.window.transcripts.audio.warning.incomplete = el audio puede estar incompleto en este segmento.

journal.window.transcripts.audio.warning.remix = el audio puede estar incompleto en este segmento.{ $slot1 }{ $stage }:{ $slot2 }{ $state }

journal.window.transcripts.awaiting.thinking.label.label.awaiting.thinking = en espera del razonamiento

journal.window.transcripts.body.context = contexto del cuerpo

journal.window.transcripts.body.entries.held =
    { $entries_quantity ->
        [one] { $entries } entrada presente
        [many] { $entries } entradas presentes
       *[other] { $entries } entradas presentes
    }

journal.window.transcripts.body.entry.count =
    { $entries_quantity ->
        [one] { $entries } entrada
        [many] { $entries } entradas
       *[other] { $entries } entradas
    }

journal.window.transcripts.body.full.day = día completo

journal.window.transcripts.body.hidden.copy.hidden.you.hid.this.day.s.body = ocultaste los datos del cuerpo de este día.

journal.window.transcripts.body.hour = hora

journal.window.transcripts.body.hour.with.data = { $hour } · { $summary }

journal.window.transcripts.body.hour.without.data = { $hour } · sin datos del cuerpo

journal.window.transcripts.body.hours.remaining =
    { $hours_quantity ->
        [one] y { $hours } hora más
        [many] y { $hours } horas más
       *[other] y { $hours } horas más
    }

journal.window.transcripts.body.panel.more.count =
    { $quantity ->
        [one] +{ $count } más en el cuerpo
        [many] +{ $count } más en el cuerpo
       *[other] +{ $count } más en el cuerpo
    }

journal.window.transcripts.body.section.brief = resumen

journal.window.transcripts.body.section.events = eventos

journal.window.transcripts.body.section.sources = fuentes

journal.window.transcripts.body.sleep.count =
    { $sessions_quantity ->
        [one] 1 sesión de sueño
        [many] { $sessions } sesiones de sueño
       *[other] { $sessions } sesiones de sueño
    }

journal.window.transcripts.body.workout.count =
    { $workouts_quantity ->
        [one] 1 entrenamiento
        [many] { $workouts } entrenamientos
       *[other] { $workouts } entrenamientos
    }

journal.window.transcripts.btn.label.failed.label.try.again = reintentar

journal.window.transcripts.btn.label.pending.label.analyze.now = analizar ahora

journal.window.transcripts.btn.title.audio.transcribe.this.segment.s.audio.and = transcribe el audio de este segmento y agrega las palabras a tu diario.

journal.window.transcripts.btn.title.image.describe.this.segment.s.photos.and = describe las fotos de este segmento y agrega las descripciones a tu diario.

journal.window.transcripts.btn.title.label.process.this.segment.and.add.the = procesa este segmento y agrega los resultados a tu diario.

journal.window.transcripts.btn.title.screen.describe.this.segment.s.screen.and = describe la pantalla de este segmento y agrega las descripciones a tu diario.

journal.window.transcripts.button.label.close = cerrar

journal.window.transcripts.button.label.open.details = abrir detalles

journal.window.transcripts.consent.stretch.content.kinds = eso es { $content_kinds }.

journal.window.transcripts.consent.stretch.copy.arriving.anything.still.arriving.from.a.device = cualquier cosa que todavía está llegando de un dispositivo

journal.window.transcripts.consent.stretch.copy.audio.audio = audio

journal.window.transcripts.consent.stretch.copy.body.body.data = datos del cuerpo

journal.window.transcripts.consent.stretch.copy.import.running.anything.from.an.import.that.s = cualquier cosa de una importación que todavía está en curso

journal.window.transcripts.consent.stretch.copy.imported.things.you.imported = cosas que importaste

journal.window.transcripts.consent.stretch.copy.in.trash.anything.from.then.that.s.already = cualquier cosa de entonces que ya está en la papelera, que conserva su propia fecha

journal.window.transcripts.consent.stretch.copy.inflight.to.move.those.delete.this.stretch = para mover eso, elimina este tramo de nuevo cuando terminen.

journal.window.transcripts.consent.stretch.copy.location.location = ubicación

journal.window.transcripts.consent.stretch.copy.made.the.entities.you.re.tending.and = las entidades que cuidas y lo que tu diario escribió sobre ellas, que todavía pueden mencionarlo

journal.window.transcripts.consent.stretch.copy.made.the.entities.you.re.tending.and.variant2 = las entidades que cuidas y lo que tu diario escribió sobre ellas, que no se borran y todavía pueden mencionarlo

journal.window.transcripts.consent.stretch.copy.madebody.activities.summaries.and.entities.made.from = actividades, resúmenes y entidades hechos a partir de eso, que todavía pueden mencionarlo

journal.window.transcripts.consent.stretch.copy.madebody.activities.summaries.and.entities.made.from.variant2 = actividades, resúmenes y entidades hechos a partir de eso, que no se borran y todavía pueden mencionarlo

journal.window.transcripts.consent.stretch.copy.madechanges.activities.made.from.it.drop.it = las actividades hechas de esto lo sueltan ahora, y los relatos de sus días y semanas salen de la búsqueda. tu diario los reescribe de lo que queda, con el razonamiento que elegiste. las entidades detectadas también lo sueltan, y las muestras de voz de esto salen de tus voces conocidas.

journal.window.transcripts.consent.stretch.copy.memories.memories.your.agents.saved.about.that = recuerdos que tus agentes guardaron sobre ese momento

journal.window.transcripts.consent.stretch.copy.other.other.things.in.your.journal = otras cosas en tu diario

journal.window.transcripts.consent.stretch.copy.processing.anything.your.journal.is.still.working = cualquier cosa en la que tu diario todavía está trabajando

journal.window.transcripts.consent.stretch.copy.reach.it.all.leaves.your.journal.now = todo sale de tu diario ahora y no se puede recuperar.

journal.window.transcripts.consent.stretch.copy.reach.it.all.leaves.your.journal.now.variant2 = todo sale de tu diario ahora y espera en la papelera como una sola cosa. después se borra. puedes recuperarlo hasta entonces.

journal.window.transcripts.consent.stretch.copy.screen.screen = pantalla

journal.window.transcripts.consent.stretch.copy.stayslead.these.stay.in.your.journal = estas se quedan en tu diario:

journal.window.transcripts.consent.stretch.copy.title.delete.a.stretch.of.time = ¿eliminar un tramo de tiempo?

journal.window.transcripts.consent.stretch.copy.title.erase.this.stretch.of.time.now = ¿borrar este tramo de tiempo ahora?

journal.window.transcripts.consent.stretch.copy.whole.day.whole.day.body.data.like.a = datos del cuerpo de todo el día, como la puntuación de sueño o de preparación, de un día que esto no cubre por completo

journal.window.transcripts.consent.stretch.erase.range = esto borra todo lo que hay en tu diario desde { $start_time } hasta { $end_time }.

journal.window.transcripts.consent.stretch.journal.zone = las horas están en la zona horaria de tu diario, { $journal_zone }.

journal.window.transcripts.consent.stretch.move.range = esto mueve todo lo que hay en tu diario desde { $start_time } hasta { $end_time }.

journal.window.transcripts.consent.trash.copy.analysis.analysis.stored.with.it = análisis guardado con esto

journal.window.transcripts.consent.trash.copy.audio.audio = audio

journal.window.transcripts.consent.trash.copy.conversation.conversation = conversación

journal.window.transcripts.consent.trash.copy.erase.if.it.s.the.last.segment = si es el último segmento de su importación, la importación, cualquier archivo que conservó y cualquiera de sus segmentos en la papelera se borran con él.

journal.window.transcripts.consent.trash.copy.erase.if.these.include.the.last.segments = si estos incluyen los últimos segmentos de una importación, la importación, cualquier archivo que conservó y sus segmentos en la papelera se borran con ellos.

journal.window.transcripts.consent.trash.copy.erasefound.detected.entities.drop.them.too.and = las entidades detectadas también los sueltan, y las muestras de voz que vengan de ellos salen de tus voces conocidas. las entidades que cuidas no se borran, y lo que tu diario escribió sobre ellas puede seguir mencionándolos.

journal.window.transcripts.consent.trash.copy.eraselead.this.erases.each.segment.s.own = esto borra lo propio de cada segmento:

journal.window.transcripts.consent.trash.copy.erasenow.erase.now = borrar ahora

journal.window.transcripts.consent.trash.copy.erasereach.they.leave.your.journal.now.and = salen de tu diario ahora y no se pueden recuperar.

journal.window.transcripts.consent.trash.copy.erasestays.activities.made.from.them.drop.them = las actividades hechas a partir de ellos los sueltan ahora, y los relatos de sus días y semanas salen de la búsqueda. tu diario luego los reescribe desde lo que queda, con el razonamiento que elegiste.

journal.window.transcripts.consent.trash.copy.found.detected.entities.drop.it.too.and = las entidades detectadas también lo sueltan, y las muestras de voz que vengan de esto salen de tus voces conocidas. las entidades que cuidas se quedan, y lo que tu diario escribió sobre ellas puede seguir mencionándolo.

journal.window.transcripts.consent.trash.copy.found.detected.entities.drop.it.too.and.variant2 = las entidades detectadas también lo sueltan, y las muestras de voz que vengan de esto salen de tus voces conocidas. las entidades que cuidas no se borran, y lo que tu diario escribió sobre ellas puede seguir mencionándolo.

journal.window.transcripts.consent.trash.copy.lead.this.erases.the.segment.s.own = esto borra lo propio del segmento:

journal.window.transcripts.consent.trash.copy.lead.this.moves.the.segment.s.own = esto mueve lo propio del segmento:

journal.window.transcripts.consent.trash.copy.location.location = ubicación

journal.window.transcripts.consent.trash.copy.many.the.files.its.import.kept.stay = los archivos que su importación conservó siguen en tu diario, porque otros segmentos todavía vienen de ellos. para quitarlos, elimina la importación.

journal.window.transcripts.consent.trash.copy.many.their.original.media.in.the.trash = su contenido multimedia original en la papelera se borra con ellos.

journal.window.transcripts.consent.trash.copy.manyone.other.segment.from.those.imports.in = también se borra 1 segmento más de esas importaciones que está en la papelera.

journal.window.transcripts.consent.trash.copy.messages.messages = mensajes

journal.window.transcripts.consent.trash.copy.move.if.it.s.the.last.segment = si es el último segmento de su importación, la importación y cualquier archivo que conservó van a la papelera con él.

journal.window.transcripts.consent.trash.copy.move.if.these.include.the.last.segments = si estos incluyen los últimos segmentos de una importación, la importación y cualquier archivo que conservó van a la papelera con ellos.

journal.window.transcripts.consent.trash.copy.movefound.detected.entities.drop.them.too.and = las entidades detectadas también los sueltan, y las muestras de voz que vengan de ellos salen de tus voces conocidas. las entidades que cuidas se quedan, y lo que tu diario escribió sobre ellas puede seguir mencionándolos.

journal.window.transcripts.consent.trash.copy.movelead.this.moves.each.segment.s.own = esto mueve lo propio de cada segmento:

journal.window.transcripts.consent.trash.copy.movereach.they.leave.your.journal.now.and = salen de tu diario ahora y esperan en la papelera; después se borran. puedes recuperar cada uno hasta entonces.

journal.window.transcripts.consent.trash.copy.movestays.activities.made.from.them.drop.them = las actividades hechas a partir de ellos los sueltan ahora, y los relatos de sus días y semanas salen de la búsqueda. tu diario luego los reescribe desde lo que queda, con el razonamiento que elegiste.

journal.window.transcripts.consent.trash.copy.movetotrash.move.to.trash = mover a la papelera

journal.window.transcripts.consent.trash.copy.none.it.s.the.last.segment.from = es el último segmento de su importación, así que su fila en tu historial de importación se borra con él.

journal.window.transcripts.consent.trash.copy.none.it.s.the.last.segment.from.variant2 = es el último segmento de su importación, así que su fila en tu historial de importación va a la papelera con él.

journal.window.transcripts.consent.trash.copy.one.it.s.the.last.segment.from = es el último segmento de su importación, así que el archivo que conservó y su fila en tu historial de importación se borran con él.

journal.window.transcripts.consent.trash.copy.one.it.s.the.last.segment.from.variant2 = es el último segmento de su importación, así que el archivo que conservó y su fila en tu historial de importación van a la papelera con él.

journal.window.transcripts.consent.trash.copy.one.its.original.media.in.the.trash = su contenido multimedia original en la papelera se borra con eso.

journal.window.transcripts.consent.trash.copy.one.the.file.import.kept.stays.in = el archivo que 1 importación conservó sigue en tu diario, porque otros segmentos todavía vienen de él. para quitarlo, elimina la importación en la página de importación.

journal.window.transcripts.consent.trash.copy.one.the.file.it.came.from.stays = el archivo del que vino sigue en tu diario, porque otros segmentos todavía vienen de él. para quitarlo, elimina la importación.

journal.window.transcripts.consent.trash.copy.one.the.import.s.other.segment.in = el otro segmento de la importación que está en la papelera se borra junto con esto.

journal.window.transcripts.consent.trash.copy.one.this.includes.the.last.of.the = esto incluye el último de los segmentos de 1 importación, así que el archivo que conservó y su fila en tu historial de importación también se borran.

journal.window.transcripts.consent.trash.copy.one.this.includes.the.last.of.the.variant2 = esto incluye el último de los segmentos de 1 importación, así que el archivo que conservó y su fila en tu historial de importación también van a la papelera.

journal.window.transcripts.consent.trash.copy.onenone.this.includes.the.last.of.the = esto incluye el último de los segmentos de 1 importación, así que su fila en tu historial de importación también se borra.

journal.window.transcripts.consent.trash.copy.onenone.this.includes.the.last.of.the.variant2 = esto incluye el último de los segmentos de 1 importación, así que su fila en tu historial de importación también va a la papelera.

journal.window.transcripts.consent.trash.copy.oneone.other.segment.from.that.import.in = también se borra 1 segmento más de esa importación que está en la papelera.

journal.window.transcripts.consent.trash.copy.openimport.open.the.import = abrir la importación

journal.window.transcripts.consent.trash.copy.photo.photo = foto

journal.window.transcripts.consent.trash.copy.reach.it.leaves.your.journal.now.and = sale de tu diario ahora y no se puede recuperar.

journal.window.transcripts.consent.trash.copy.reach.it.leaves.your.journal.now.and.variant2 = sale de tu diario ahora y espera en la papelera. después se borra. puedes recuperarlo hasta entonces.

journal.window.transcripts.consent.trash.copy.screen.screen.frames = fotogramas de pantalla

journal.window.transcripts.consent.trash.copy.screenshot.screenshot = captura de pantalla

journal.window.transcripts.consent.trash.copy.stays.activities.made.from.it.drop.it = las actividades hechas a partir de esto lo sueltan ahora, y los relatos de su día y su semana salen de la búsqueda. tu diario luego los reescribe desde lo que queda, con el razonamiento que elegiste.

journal.window.transcripts.consent.trash.copy.title.delete.segment = ¿eliminar el segmento?

journal.window.transcripts.consent.trash.copy.title.erase.this.segment.now = ¿borrar este segmento ahora?

journal.window.transcripts.consent.trash.copy.transcript.transcript = transcripción

journal.window.transcripts.consent.trash.copy.unknown.any.file.its.import.kept.stays = cualquier archivo que su importación conservó sigue en tu diario, porque otros segmentos todavía vienen de él. para quitarlo, elimina la importación.

journal.window.transcripts.consent.trash.copy.unknown.it.s.the.last.segment.from = es el último segmento de su importación, así que cualquier archivo que conservó y su fila en tu historial de importación se borran con él.

journal.window.transcripts.consent.trash.copy.unknown.it.s.the.last.segment.from.variant2 = es el último segmento de su importación, así que cualquier archivo que conservó y su fila en tu historial de importación van a la papelera con él.

journal.window.transcripts.consent.trash.import.erase.files.count =
    { $retained_files_quantity ->
        [one] es el último segmento de su importación, así que el { $retained_files } archivo que conservó y su fila en tu historial de importación se borran con él.
        [many] es el último segmento de su importación, así que los { $retained_files } archivos que conservó y su fila en tu historial de importación se borran con él.
       *[other] es el último segmento de su importación, así que los { $retained_files } archivos que conservó y su fila en tu historial de importación se borran con él.
    }

journal.window.transcripts.consent.trash.import.erase.named = es el último segmento de { $import_name }, así que ese archivo y su fila en tu historial de importación se borran con él.

journal.window.transcripts.consent.trash.import.move.files.count =
    { $retained_files_quantity ->
        [one] es el último segmento de su importación, así que el { $retained_files } archivo que conservó y su fila en tu historial de importación van a la papelera con él.
        [many] es el último segmento de su importación, así que los { $retained_files } archivos que conservó y su fila en tu historial de importación van a la papelera con él.
       *[other] es el último segmento de su importación, así que los { $retained_files } archivos que conservó y su fila en tu historial de importación van a la papelera con él.
    }

journal.window.transcripts.consent.trash.import.move.named = es el último segmento de { $import_name }, así que ese archivo y su fila en tu historial de importación van a la papelera con él.

journal.window.transcripts.consent.trash.import.pending.count =
    { $waiting_segments_quantity ->
        [one] el { $waiting_segments } segmento más de la importación que está en la papelera se borra junto con esto.
        [many] los { $waiting_segments } segmentos más de la importación que están en la papelera se borran junto con esto.
       *[other] los { $waiting_segments } segmentos más de la importación que están en la papelera se borran junto con esto.
    }

journal.window.transcripts.consent.trash.import.set.erase.named = esto incluye el último de los segmentos de { $import_name }, así que ese archivo y su fila en tu historial de importación también se borran.

journal.window.transcripts.consent.trash.import.set.move.named = esto incluye el último de los segmentos de { $import_name }, así que ese archivo y su fila en tu historial de importación también van a la papelera.

journal.window.transcripts.consent.trash.import.set.pending.count =
    { $waiting_segments_quantity ->
        [one] también se borra { $waiting_segments } segmento más de esa importación que está en la papelera.
        [many] también se borran { $waiting_segments } segmentos más de esa importación que están en la papelera.
       *[other] también se borran { $waiting_segments } segmentos más de esa importación que están en la papelera.
    }

journal.window.transcripts.consent.trash.import.stay.named = { $import_name }, el archivo del que vino, sigue en tu diario, porque otros segmentos todavía vienen de él. para quitarlo, elimina la importación.

journal.window.transcripts.consent.trash.imports.set.erase.count =
    { $imports_quantity ->
        [one] esto incluye el último de los segmentos de { $imports } importación, así que los archivos que conservó y su fila en tu historial de importación también se borran.
        [many] esto incluye el último de los segmentos de { $imports } importaciones, así que los archivos que conservaron y sus filas en tu historial de importación también se borran.
       *[other] esto incluye el último de los segmentos de { $imports } importaciones, así que los archivos que conservaron y sus filas en tu historial de importación también se borran.
    }

journal.window.transcripts.consent.trash.imports.set.erase.without.files.count =
    { $imports_quantity ->
        [one] esto incluye el último de los segmentos de { $imports } importación, así que su fila en tu historial de importación también se borra.
        [many] esto incluye el último de los segmentos de { $imports } importaciones, así que sus filas en tu historial de importación también se borran.
       *[other] esto incluye el último de los segmentos de { $imports } importaciones, así que sus filas en tu historial de importación también se borran.
    }

journal.window.transcripts.consent.trash.imports.set.move.count =
    { $imports_quantity ->
        [one] esto incluye el último de los segmentos de { $imports } importación, así que los archivos que conservó y su fila en tu historial de importación también van a la papelera.
        [many] esto incluye el último de los segmentos de { $imports } importaciones, así que los archivos que conservaron y sus filas en tu historial de importación también van a la papelera.
       *[other] esto incluye el último de los segmentos de { $imports } importaciones, así que los archivos que conservaron y sus filas en tu historial de importación también van a la papelera.
    }

journal.window.transcripts.consent.trash.imports.set.move.without.files.count =
    { $imports_quantity ->
        [one] esto incluye el último de los segmentos de { $imports } importación, así que su fila en tu historial de importación también va a la papelera.
        [many] esto incluye el último de los segmentos de { $imports } importaciones, así que sus filas en tu historial de importación también van a la papelera.
       *[other] esto incluye el último de los segmentos de { $imports } importaciones, así que sus filas en tu historial de importación también van a la papelera.
    }

journal.window.transcripts.consent.trash.imports.set.pending.count =
    { $waiting_segments_quantity ->
        [one] también se borra { $waiting_segments } segmento más de esas importaciones que está en la papelera.
        [many] también se borran { $waiting_segments } segmentos más de esas importaciones que están en la papelera.
       *[other] también se borran { $waiting_segments } segmentos más de esas importaciones que están en la papelera.
    }

journal.window.transcripts.consent.trash.imports.stay.count =
    { $imports_quantity ->
        [one] los archivos que { $imports } importación conservó siguen en tu diario, porque otros segmentos todavía vienen de ella. para quitarlos, elimina la importación en la página de importación.
        [many] los archivos que { $imports } importaciones conservaron siguen en tu diario, porque otros segmentos todavía vienen de ellas. para quitar uno, elimina su importación en la página de importación.
       *[other] los archivos que { $imports } importaciones conservaron siguen en tu diario, porque otros segmentos todavía vienen de ellas. para quitar uno, elimina su importación en la página de importación.
    }

journal.window.transcripts.consent.trash.imports.stay.named = { $import_name } sigue en tu diario, porque otros segmentos todavía vienen de ahí. para quitar eso, elimina su importación en la página de importación.

journal.window.transcripts.consent.trash.media.size = tamaño actual del contenido multimedia sin procesar: { $media_size }

journal.window.transcripts.consent.trash.selection.erase.title =
    { $segments_quantity ->
        [one] ¿borrar { $segments } segmento ahora?
        [many] ¿borrar { $segments } segmentos ahora?
       *[other] ¿borrar { $segments } segmentos ahora?
    }

journal.window.transcripts.consent.trash.selection.more =
    { $segments_quantity ->
        [one] y { $segments } más
        [many] y { $segments } más
       *[other] y { $segments } más
    }

journal.window.transcripts.consent.trash.selection.move.title =
    { $segments_quantity ->
        [one] ¿eliminar { $segments } segmento?
        [many] ¿eliminar { $segments } segmentos?
       *[other] ¿eliminar { $segments } segmentos?
    }

journal.window.transcripts.conversation.reason.insufficient.evidence = el archivo del que vino no muestra si se habló o se escribió.

journal.window.transcripts.conversation.reason.origin.unknown = tu diario no puede saber de qué archivo vino.

journal.window.transcripts.conversation.reason.original.missing = el archivo del que vino no está en tu diario.

journal.window.transcripts.conversation.unresolved = tu diario no puede saber si esta conversación se habló o se escribió.

journal.window.transcripts.conversation.why = por qué

journal.window.transcripts.date.last.weekday = El{ $slot1 }{ $weekday } pasado

journal.window.transcripts.date.narrow.friday = V

journal.window.transcripts.date.narrow.monday = L

journal.window.transcripts.date.narrow.saturday = S

journal.window.transcripts.date.narrow.sunday = D

journal.window.transcripts.date.narrow.thursday = J

journal.window.transcripts.date.narrow.tuesday = M

journal.window.transcripts.date.narrow.wednesday = M

journal.window.transcripts.date.next.week = semana siguiente

journal.window.transcripts.date.previous.week = semana anterior

journal.window.transcripts.date.today = hoy

journal.window.transcripts.date.today.narrow = H

journal.window.transcripts.date.years = Años

journal.window.transcripts.date.yesterday = ayer

journal.window.transcripts.day.latest = lo último de { $device } a las { $time }

journal.window.transcripts.day.load.error.status = los datos del día fallaron:{ $slot1 }{ $status }

journal.window.transcripts.day.load.retry = Reintentar

journal.window.transcripts.disclosure.label.recent.raw.signal.rows = filas recientes de señal sin procesar

journal.window.transcripts.entries.quantity =
    { $items_quantity ->
        [one] entradas de transcripción, { $items } elemento
        [many] entradas de transcripción, { $items } elementos
       *[other] entradas de transcripción, { $items } elementos
    }

journal.window.transcripts.entries.show.remaining =
    { $remaining_quantity ->
        [one] mostrar el resto de este bloque ({ $remaining } más) →
        [many] mostrar el resto de este bloque ({ $remaining } más) →
       *[other] mostrar el resto de este bloque ({ $remaining } más) →
    }

journal.window.transcripts.entries.shown =
    { $shown_quantity ->
        [one] entradas de transcripción, { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } mostradas
        [many] entradas de transcripción, { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } mostradas
       *[other] entradas de transcripción, { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } mostradas
    }

journal.window.transcripts.entry.play.from.time = reproducir desde { $time }

journal.window.transcripts.entry.possible.name = (nombre posible: { $entity_name })

journal.window.transcripts.field.label.segments.in.this.range = segmentos en este rango

journal.window.transcripts.granola.copy.attendees.attendees = asistentes

journal.window.transcripts.granola.copy.calendar.calendar.event = evento del calendario

journal.window.transcripts.granola.copy.heading.granola.s.notes.on.this.meeting = notas de { $slot1 } sobre esta reunión

journal.window.transcripts.granola.copy.notranscript.granola.has.no.transcript.for.this = { $slot1 } no tiene transcripción para esta reunión.

journal.window.transcripts.granola.copy.open.open.in.granola = abrir en { $slot1 }

journal.window.transcripts.granola.copy.privatenotes.your.notes = tus notas

journal.window.transcripts.granola.copy.speakerassistive.as.granola.labelled.the.speaker = como { $slot1 } etiquetó al hablante

journal.window.transcripts.granola.copy.summary.summary = resumen

journal.window.transcripts.kind.audio.and.screen = audio y pantalla

journal.window.transcripts.kind.audio.plus.screen = audio + pantalla

journal.window.transcripts.kind.desktop.activity = actividad de escritorio

journal.window.transcripts.kind.location = ubicación

journal.window.transcripts.kind.pages = páginas

journal.window.transcripts.kind.photos = fotos

journal.window.transcripts.link.label.open.full.body.day = abrir el día completo del cuerpo

journal.window.transcripts.location.only.readings =
    { $readings_quantity ->
        [one] este segmento guarda solo lugar: 1 lectura.
        [many] este segmento guarda solo lugar: { $readings } lecturas.
       *[other] este segmento guarda solo lugar: { $readings } lecturas.
    }

journal.window.transcripts.location.row.readings =
    { $readings_quantity ->
        [one] { $start_time } · 1 lectura
        [many] { $start_time } · { $readings } lecturas
       *[other] { $start_time } · { $readings } lecturas
    }

journal.window.transcripts.location.show.count =
    { $segments_quantity ->
        [one] mostrar 1 segmento de lugar
        [many] mostrar { $segments } segmentos de lugar
       *[other] mostrar { $segments } segmentos de lugar
    }

journal.window.transcripts.media.audio.duration = audio del segmento ·{ $slot1 }{ $duration }

journal.window.transcripts.media.audio.format.unsupported = Formato no compatible

journal.window.transcripts.media.audio.label = audio del segmento

journal.window.transcripts.media.frame.count =
    { $frames_quantity ->
        [one] 1 fotograma
        [many] { $frames } fotogramas
       *[other] { $frames } fotogramas
    }

journal.window.transcripts.media.frames.decoding =
    { $decoded_quantity ->
        [one] Decodificando fotogramas clave { $decoded }/{ $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } ({ $percent }%)…
        [many] Decodificando fotogramas clave { $decoded }/{ $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } ({ $percent }%)…
       *[other] Decodificando fotogramas clave { $decoded }/{ $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        } ({ $percent }%)…
    }

journal.window.transcripts.media.original.audio = el audio original de este segmento

journal.window.transcripts.media.original.audio.waiting = el audio original de este segmento está en la papelera

journal.window.transcripts.media.original.screen = el contenido multimedia de pantalla original de este segmento

journal.window.transcripts.media.original.screen.waiting = el contenido multimedia de pantalla original de este segmento está en la papelera

journal.window.transcripts.media.pending.duration = duración{ $slot1 }{ $value }

journal.window.transcripts.media.pending.raw.media = contenido multimedia sin procesar{ $slot1 }{ $value }

journal.window.transcripts.media.removal.import = la grabación no se conservó cuando la importaste

journal.window.transcripts.media.removal.offload = eliminaste{ $slot1 }{ $what }{ $slot2 }después de que tu copia de seguridad copió eso

journal.window.transcripts.media.removal.owner = eliminaste{ $slot1 }{ $what }

journal.window.transcripts.media.removal.owner.voice = tu diario eliminó{ $slot1 }{ $what }{ $slot2 }después de guardar tus palabras

journal.window.transcripts.media.removal.policy = eliminaste{ $slot1 }{ $what }{ $slot2 }después de que tu configuración de retención marcó eso

journal.window.transcripts.media.removal.unavailable = { $what }{ $slot1 }ya no está en tu diario

journal.window.transcripts.media.screen.activity = actividad de pantalla

journal.window.transcripts.media.screen.image = imagen de pantalla

journal.window.transcripts.media.size.audio = audio{ $slot1 }{ $size }

journal.window.transcripts.media.size.photo = foto{ $slot1 }{ $size }

journal.window.transcripts.media.size.screen = pantalla{ $slot1 }{ $size }

journal.window.transcripts.media.trash.copy.desc.you.can.recover.it.there.until = puedes recuperar eso ahí hasta que se borre.

journal.window.transcripts.media.trash.copy.link.open.trash = abrir la papelera

journal.window.transcripts.media.video.error.code = código{ $slot1 }{ $code }

journal.window.transcripts.message.assistant = Asistente

journal.window.transcripts.message.show.from.start = mostrar desde el inicio

journal.window.transcripts.message.to = para { $recipients }

journal.window.transcripts.message.to.everyone = para todo el mundo

journal.window.transcripts.message.to.only = solo para { $recipient }

journal.window.transcripts.message.truncated = el resto de este mensaje no se conservó.

journal.window.transcripts.message.unknown = Desconocido

journal.window.transcripts.message.words.before =
    { $words_quantity ->
        [one] { $words } palabra antes de este pasaje
        [many] { $words } palabras antes de este pasaje
       *[other] { $words } palabras antes de este pasaje
    }

journal.window.transcripts.message.you = tú

journal.window.transcripts.notice.age.days = { $count } d

journal.window.transcripts.notice.age.hours = { $count } h

journal.window.transcripts.notice.age.minutes = { $count } min

journal.window.transcripts.option.label.choose.a.segment = elegir un segmento

journal.window.transcripts.owner.desc.no.observations.were.found.for.this = no se recibió nada para este día

journal.window.transcripts.owner.desc.now.pick.a.block.in.the = ahora elige un bloque en la columna de detalle para leerlo

journal.window.transcripts.owner.desc.pick.part.of.the.day.on = elige una parte del día a la izquierda y luego un bloque en el detalle para leerlo.

journal.window.transcripts.owner.desc.reload.to.try.again = recarga para volver a intentarlo.

journal.window.transcripts.owner.desc.something.went.wrong.decoding.the.screen = algo salió mal al decodificar los datos de pantalla. vuelve a elegir el segmento.

journal.window.transcripts.owner.desc.something.went.wrong.loading.the.transcript = algo salió mal al cargar la transcripción. vuelve a elegir el segmento, o actualiza la página.

journal.window.transcripts.owner.desc.this.segment.has.no.audio.content = este segmento no tiene contenido de audio

journal.window.transcripts.owner.desc.this.segment.has.no.imported.mentra = este segmento no tiene telemetría importada de { $slot1 }

journal.window.transcripts.owner.desc.this.segment.has.no.page.content = este segmento no tiene contenido de página

journal.window.transcripts.owner.desc.this.segment.has.no.photos = este segmento no tiene fotos

journal.window.transcripts.owner.desc.this.segment.has.no.screen.observations = este segmento no tiene contenido de pantalla recibido

journal.window.transcripts.owner.desc.this.segment.has.no.transcript.content = este segmento no tiene contenido de transcripción

journal.window.transcripts.owner.desc.try.again.in.a.moment = vuelve a intentarlo en un momento

journal.window.transcripts.owner.heading.couldn.t.load.body.context = no se pudo cargar el contexto del cuerpo

journal.window.transcripts.owner.heading.couldn.t.load.screen.entries = no se pudieron cargar las entradas de pantalla

journal.window.transcripts.owner.heading.couldn.t.load.this.segment = no se pudo cargar este segmento

journal.window.transcripts.owner.heading.no.audio.entries = no hay entradas de audio

journal.window.transcripts.owner.heading.no.page.entries = no hay entradas de página

journal.window.transcripts.owner.heading.no.photos = no hay fotos

journal.window.transcripts.owner.heading.no.screen.entries = no hay entradas de pantalla

journal.window.transcripts.owner.heading.no.signal.context = no hay contexto de señal

journal.window.transcripts.owner.heading.no.transcript.entries = no hay entradas de transcripción

journal.window.transcripts.owner.heading.nothing.here = no hay nada aquí

journal.window.transcripts.owner.heading.transcripts.couldn.t.be.loaded = no se pudieron cargar las transcripciones.

journal.window.transcripts.owner.heading.your.day.at.a.glance = tu día de un vistazo

journal.window.transcripts.owner.status.or.action.couldn.t.copy.stream.id = no se pudo copiar el id del flujo

journal.window.transcripts.owner.status.or.action.stream.id.copied = id del flujo copiado

journal.window.transcripts.photo.copy.captionlabel.your.note = tu nota

journal.window.transcripts.photo.copy.descriptionlabel.generated.description = descripción generada

journal.window.transcripts.photo.copy.label.photo = foto

journal.window.transcripts.photo.copy.noprocessing.thinking.isn.t.set.up.yet = el razonamiento todavía no está configurado, así que esta foto no se ha descrito.

journal.window.transcripts.photo.copy.noprocessingaction.set.up.thinking = configurar el razonamiento

journal.window.transcripts.photo.copy.unavailable.this.photo.couldn.t.be.described = no se pudo describir esta foto. la foto sigue en tu diario.

journal.window.transcripts.photo.copy.uploadfallback.no.date.in.photo.placed.at = sin fecha en la foto · colocada a la hora de la importación

journal.window.transcripts.photo.copy.viewerlabel.photo.viewer = visor de fotos

journal.window.transcripts.raw.json.summary = { $json_label } sin procesar, { $summary }

journal.window.transcripts.response.detail.day.not.found = No se encontró el día

journal.window.transcripts.response.detail.day.stream.and.segment.name.one = el día, el flujo y el segmento nombran un segmento

journal.window.transcripts.response.detail.failed.to.create.analysis.marker.detail = No se pudo crear el marcador de análisis:{ $slot1 }{ $detail }

journal.window.transcripts.response.detail.failed.to.start.analysis.detail = No se pudo iniciar el análisis:{ $slot1 }{ $detail }

journal.window.transcripts.response.detail.file.not.found = No se encontró el archivo

journal.window.transcripts.response.detail.invalid.day.format = Formato de día no válido

journal.window.transcripts.response.detail.invalid.delete.body = Cuerpo de eliminación no válido

journal.window.transcripts.response.detail.invalid.month.format = Formato de mes no válido

journal.window.transcripts.response.detail.invalid.segment.in.the.set = Segmento no válido en el conjunto

journal.window.transcripts.response.detail.invalid.segment.key.format = Formato de clave de segmento no válido

journal.window.transcripts.response.detail.invalid.segment.path = Ruta de segmento no válida

journal.window.transcripts.response.detail.invalid.stream.format = Formato de flujo no válido

journal.window.transcripts.response.detail.invalid.stretch.body = Cuerpo de tramo no válido

journal.window.transcripts.response.detail.modality.must.be.audio.screen.or = la modalidad debe ser { $slot1 }

journal.window.transcripts.response.detail.raw.media.is.no.longer.available = El contenido multimedia sin procesar ya no está disponible

journal.window.transcripts.response.detail.saying.who.said.a.line.needs = decir quién dijo una línea requiere al propietario

journal.window.transcripts.response.detail.segment.directory.not.found = No se encontró el directorio del segmento

journal.window.transcripts.response.detail.segment.modality.is.already.analyzed = La modalidad del segmento ya está analizada

journal.window.transcripts.response.detail.segment.not.found = No se encontró el segmento

journal.window.transcripts.response.detail.source.and.sentence.id.name.the = la fuente y { $slot1 } nombran la línea

journal.window.transcripts.response.detail.speaker.is.kind.owner.kind.person = el hablante es { $slot1 }, { $slot2 } o { $slot3 }

journal.window.transcripts.response.detail.the.delete.task.stopped = la tarea de eliminación se detuvo

journal.window.transcripts.response.detail.the.preview.task.stopped = la tarea de vista previa se detuvo

journal.window.transcripts.response.detail.the.stretch.holds.no.segment = el tramo no guarda ningún segmento

journal.window.transcripts.response.detail.the.stretch.task.stopped = la tarea del tramo se detuvo

journal.window.transcripts.response.detail.unregistered.media.extension = Extensión de contenido multimedia no registrada

journal.window.transcripts.response.detail.workout.not.found = No se encontró el entrenamiento

journal.window.transcripts.response.error.analysis.couldn.t.run.because.the = no se pudo ejecutar el análisis: el contenido multimedia sin procesar ya no está disponible.

journal.window.transcripts.response.error.choose.days.or.less = elige 31 días o menos.

journal.window.transcripts.response.error.nothing.from.that.time.could.go = nada de ese momento pudo ir a la papelera.

journal.window.transcripts.response.error.one.of.those.values.couldn.t = no se pudo usar uno de esos valores.

journal.window.transcripts.response.error.only.you.can.say.who.said = solo tú puedes decir quién dijo una línea.

journal.window.transcripts.response.error.reason.nothing.was.deleted = { $reason }. no se eliminó nada.

journal.window.transcripts.response.error.that.action.isn.t.available.in = esa acción no está disponible en el estado actual.

journal.window.transcripts.response.error.that.day.couldn.t.be.used = no se pudo usar ese día.

journal.window.transcripts.response.error.that.delete.couldn.t.be.read = no se pudo leer esa eliminación, así que no se eliminó nada.

journal.window.transcripts.response.error.that.file.couldn.t.be.read = no se pudo leer ese archivo.

journal.window.transcripts.response.error.that.file.isn.t.available = ese archivo no está disponible.

journal.window.transcripts.response.error.that.month.couldn.t.be.used = no se pudo usar ese mes.

journal.window.transcripts.response.error.that.path.couldn.t.be.used = no se pudo usar esa ruta.

journal.window.transcripts.response.error.that.request.didn.t.finish = esa solicitud no terminó.

journal.window.transcripts.response.error.that.segment.or.stream.couldn.t = ese segmento o flujo no se pudo usar.

journal.window.transcripts.response.error.that.stretch.couldn.t.be.read = no se pudo leer ese tramo, así que no se eliminó nada.

journal.window.transcripts.response.error.that.workout.isn.t.in.your = ese entrenamiento no está en tu diario.

journal.window.transcripts.response.error.the.end.has.to.be.after = el fin tiene que ser posterior al inicio.

journal.window.transcripts.response.error.who.said.that.line.couldn.t = no se pudo guardar quién dijo esa línea.

journal.window.transcripts.response.error.your.journal.couldn.t.finish.that = tu diario no pudo terminar esa eliminación. recarga para revisar.

journal.window.transcripts.response.error.your.journal.couldn.t.move.that = tu diario no pudo mover eso a la papelera, así que no se eliminó nada.

journal.window.transcripts.response.error.your.journal.couldn.t.move.that.variant2 = tu diario no pudo mover eso a la papelera.

journal.window.transcripts.response.error.your.journal.couldn.t.read.what = tu diario no pudo leer qué se llevaría esa eliminación.

journal.window.transcripts.response.set.member.failed = tu diario no pudo moverlo a la papelera.

journal.window.transcripts.response.set.member.held = { $reason }.

journal.window.transcripts.response.set.member.missing = ese segmento no está en tu diario.

journal.window.transcripts.screenshot.copy.j1.screenshot = captura de pantalla

journal.window.transcripts.screenshot.copy.j3.the.text.in.this.screenshot.isn = el texto de esta captura de pantalla todavía no se puede buscar. las de hoy van primero y después las anteriores.

journal.window.transcripts.screenshot.copy.j4.screenshots = capturas de pantalla

journal.window.transcripts.screenshot.copy.j5.screenshot = captura de pantalla

journal.window.transcripts.screenshot.copy.j6.action.set.up.thinking = configurar el razonamiento

journal.window.transcripts.screenshot.copy.j6.thinking.isn.t.set.up.yet = todavía no hay razonamiento configurado, así que el texto de esta captura de pantalla no se puede hacer buscable.

journal.window.transcripts.screenshot.copy.j7.the.text.in.this.screenshot.couldn = no se pudo hacer buscable el texto de esta captura de pantalla. la captura de pantalla se queda en tu diario.

journal.window.transcripts.section.heading.body.context = contexto del cuerpo

journal.window.transcripts.section.heading.location = ubicación

journal.window.transcripts.section.heading.transcripts = transcripciones

journal.window.transcripts.segment.range.entries =
    { $entries_quantity ->
        [one] { $range } · { $entries } entrada
        [many] { $range } · { $entries } entradas
       *[other] { $range } · { $entries } entradas
    }

journal.window.transcripts.show.everything = todo

journal.window.transcripts.show.label = mostrar

journal.window.transcripts.show.left.out =
    { $conversations_quantity ->
        [one] { $conversations } conversación solo se muestra en { $everything_start }todo{ $everything_end }: tu diario no puede distinguir si fue hablada o escrita.
        [many] { $conversations } conversaciones solo se muestran en { $everything_start }todo{ $everything_end }: tu diario no puede distinguir si fueron habladas o escritas.
       *[other] { $conversations } conversaciones solo se muestran en { $everything_start }todo{ $everything_end }: tu diario no puede distinguir si fueron habladas o escritas.
    }

journal.window.transcripts.show.messages = mensajes

journal.window.transcripts.show.transcripts = transcripciones

journal.window.transcripts.signals.calendar.hidden.count =
    { $quantity ->
        [one] { $count } evento de calendario deduplicado más, oculto en esta vista compacta
        [many] { $count } eventos de calendario deduplicados más, ocultos en esta vista compacta
       *[other] { $count } eventos de calendario deduplicados más, ocultos en esta vista compacta
    }

journal.window.transcripts.signals.calendar.repeated =
    { $visits_quantity ->
        [one] { $range } · se vio { $visits }x
        [many] { $range } · se vio { $visits }x
       *[other] { $range } · se vio { $visits }x
    }

journal.window.transcripts.signals.no = no

journal.window.transcripts.signals.row.accuracy = precisión

journal.window.transcripts.signals.row.button = botón

journal.window.transcripts.signals.row.camera = cámara

journal.window.transcripts.signals.row.camera.resolution = resolución de la cámara

journal.window.transcripts.signals.row.charging = en carga

journal.window.transcripts.signals.row.latest = más reciente

journal.window.transcripts.signals.row.latitude = latitud

journal.window.transcripts.signals.row.level = nivel

journal.window.transcripts.signals.row.longitude = longitud

journal.window.transcripts.signals.row.microphone = micrófono

journal.window.transcripts.signals.row.raw.rows = filas en bruto

journal.window.transcripts.signals.row.speaker = hablante

journal.window.transcripts.signals.row.unique.events = eventos únicos

journal.window.transcripts.signals.row.updates = actualizaciones

journal.window.transcripts.signals.row.video.recording = grabación de video

journal.window.transcripts.signals.row.wifi = wifi

journal.window.transcripts.signals.section.calendar.snapshot = instantánea del calendario

journal.window.transcripts.signals.section.device.capabilities = capacidades del dispositivo

journal.window.transcripts.signals.section.glasses.battery = batería de las gafas

journal.window.transcripts.signals.section.location = ubicación

journal.window.transcripts.signals.yes = sí

journal.window.transcripts.speaker.chip.maybe.name = ¿tal vez { $speaker_name }?

journal.window.transcripts.speaker.chip.maybe.owner = ¿tal vez tú?

journal.window.transcripts.speaker.chip.probable.name = probablemente { $speaker_name }

journal.window.transcripts.speaker.chip.probable.owner = probablemente tú

journal.window.transcripts.speaker.chip.voice.number = voz { $voice_number }

journal.window.transcripts.speaker.copy.action.unavailable = cambio de hablante no disponible

journal.window.transcripts.speaker.copy.already.correct = ya definido

journal.window.transcripts.speaker.copy.assign.label = agregar hablante

journal.window.transcripts.speaker.copy.change.label = cambiar hablante

journal.window.transcripts.speaker.copy.confidence.high = confianza alta

journal.window.transcripts.speaker.copy.confidence.unknown = confianza no disponible

journal.window.transcripts.speaker.copy.correct.busy = los archivos de hablante están ocupados

journal.window.transcripts.speaker.copy.correct.retry = reintentar el cambio de hablante

journal.window.transcripts.speaker.copy.hedge.maybe = ¿tal vez { $slot1 }?

journal.window.transcripts.speaker.copy.hedge.probable = probablemente { $slot1 }

journal.window.transcripts.speaker.copy.line.changed = esta línea cambió desde que se mostró. vuelve a cargar para verla como está ahora.

journal.window.transcripts.speaker.copy.line.only = solo esta línea

journal.window.transcripts.speaker.copy.line.only.body = tu respuesta es para esta línea, no para su voz. para enseñarle una voz a tu diario, o ordenar las voces que conoce, ve a { $slot1 }.

journal.window.transcripts.speaker.copy.line.only.link = hablantes

journal.window.transcripts.speaker.copy.line.owner = tu diario todavía no sabe qué persona eres, así que no puede marcar las líneas como tuyas.

journal.window.transcripts.speaker.copy.line.person = no se puede elegir a esa persona. elige a alguien más, o a alguien que no conoces.

journal.window.transcripts.speaker.copy.line.refused = no se pudo guardar esa respuesta

journal.window.transcripts.speaker.copy.line.source = esta línea no admite una respuesta sobre quién la dijo

journal.window.transcripts.speaker.copy.line.unreadable = no se pueden leer tus respuestas anteriores sobre quién habló aquí, así que esta no se guardó

journal.window.transcripts.speaker.copy.margin.acoustic = coincidencia cercana de voz

journal.window.transcripts.speaker.copy.margin.owner = coincidencia cercana de propietario

journal.window.transcripts.speaker.copy.no.embedding = muestra de voz no disponible

journal.window.transcripts.speaker.copy.owner.identity.required = configura tu identidad antes de etiquetarte

journal.window.transcripts.speaker.copy.owner.name = tú

journal.window.transcripts.speaker.copy.owner.set = tu respuesta

journal.window.transcripts.speaker.copy.owner.too.close = esa voz se parece demasiado a la tuya para guardarla ahí

journal.window.transcripts.speaker.copy.owner.unknown.chip = alguien que no conoces

journal.window.transcripts.speaker.copy.picker.empty = todavía no hay personas en tu diario

journal.window.transcripts.speaker.copy.picker.guess = volver a lo que supone tu diario

journal.window.transcripts.speaker.copy.picker.no.results = no hay personas que coincidan

journal.window.transcripts.speaker.copy.picker.owner = yo

journal.window.transcripts.speaker.copy.picker.search.placeholder = buscar una persona

journal.window.transcripts.speaker.copy.picker.title = elegir hablante

journal.window.transcripts.speaker.copy.picker.unknown = alguien que no conozco

journal.window.transcripts.speaker.copy.propagation.dismiss = descartar

journal.window.transcripts.speaker.copy.someone.else = enseñar esta voz…

journal.window.transcripts.speaker.copy.unknown.chip = voz desconocida

journal.window.transcripts.speaker.copy.voice.assistive = una voz sin nombre dondequiera que aparezca este número

journal.window.transcripts.speaker.copy.voice.chip = voz { $slot1 }

journal.window.transcripts.speaker.copy.voice.named.assistive = coincide con una voz que nombraste

journal.window.transcripts.speaker.copy.voice.named.count = { $slot1 } oraciones ahora muestran este nombre

journal.window.transcripts.speaker.copy.voice.named.one = 1 oración ahora muestra este nombre

journal.window.transcripts.speaker.copy.voice.scope = dondequiera que aparezca esta voz

journal.window.transcripts.speaker.copy.voice.scope.note = no se puede deshacer

journal.window.transcripts.speaker.line.only.help = tu respuesta es para esta línea, no para su voz. para enseñarle una voz a tu diario, o ordenar las voces que conoce, ve a { $link_start }hablantes{ $link_end }.

journal.window.transcripts.speaker.named.sentences =
    { $sentences_quantity ->
        [one] 1 oración ahora muestra este nombre
        [many] { $sentences } oraciones ahora muestran este nombre
       *[other] { $sentences } oraciones ahora muestran este nombre
    }

journal.window.transcripts.speaker.warning.ambiguous = la fuente de la etiqueta de hablante es ambigua para este segmento

journal.window.transcripts.speaker.warning.unavailable = las etiquetas de hablante no están disponibles para este segmento

journal.window.transcripts.speaker.warning.unreadable = no se pueden leer tus respuestas sobre quién habló, así que no se nombra a nadie en este segmento

journal.window.transcripts.status.text.hide.location.segments = ocultar segmentos de lugar

journal.window.transcripts.status.text.raw.json = { $slot1 } en bruto

journal.window.transcripts.status.text.screen.image = imagen de pantalla

journal.window.transcripts.status.text.untitled.event = Evento sin título

journal.window.transcripts.stretch.copy.backwards.the.end.has.to.be.after = el fin tiene que ser posterior al inicio.

journal.window.transcripts.stretch.copy.button.delete.a.stretch.of.time = eliminar un tramo de tiempo

journal.window.transcripts.stretch.copy.checking.checking.what.s.in.your.journal = revisando qué hay en tu diario de ese momento…

journal.window.transcripts.stretch.copy.emptyleft.nothing.from.that.time.can.go = nada de ese momento puede ir a la papelera.

journal.window.transcripts.stretch.copy.emptynone.your.journal.has.nothing.from.that = tu diario no tiene nada de ese momento.

journal.window.transcripts.stretch.copy.failed.couldn.t.check.what.s.in = no se pudo revisar qué hay en tu diario de ese momento.

journal.window.transcripts.stretch.copy.from.from = de

journal.window.transcripts.stretch.copy.heading.this.segment.is.in.the.trash = este segmento está en la papelera

journal.window.transcripts.stretch.copy.message.it.s.back.in.your.journal = volvió a tu diario.

journal.window.transcripts.stretch.copy.message.it.s.still.moving.into.the = todavía se está moviendo a la papelera. vuelve a intentarlo desde ahí cuando termine.

journal.window.transcripts.stretch.copy.message.nothing.from.that.time.could.go = nada de ese momento pudo ir a la papelera.

journal.window.transcripts.stretch.copy.message.part.of.it.comes.from.an = una parte viene de una importación que está en la papelera. recupera esa primero.

journal.window.transcripts.stretch.copy.message.some.of.it.couldn.t.be = una parte no se pudo mover y sigue en tu diario. vuelve a eliminar este tramo para mover el resto.

journal.window.transcripts.stretch.copy.retry.try.again = reintentar

journal.window.transcripts.stretch.copy.title.couldn.t.recover.that = no se pudo recuperar eso

journal.window.transcripts.stretch.copy.title.couldn.t.recover.that.yet = no se pudo recuperar eso todavía

journal.window.transcripts.stretch.copy.title.nothing.was.moved = no se movió nada

journal.window.transcripts.stretch.copy.to.to = a

journal.window.transcripts.stretch.copy.toolong.choose.days.or.less = elige 31 días o menos.

journal.window.transcripts.stretch.delete.start.label = eliminar un tramo de tiempo a partir de { $day }

journal.window.transcripts.stretch.receipt.erase.date = se borra el { $erase_date }, a menos que lo recuperes.

journal.window.transcripts.stretch.receipt.erase.range = se borró { $date_range } de tu diario

journal.window.transcripts.stretch.receipt.move.range = se movió { $date_range } a la papelera

journal.window.transcripts.stretch.receipt.recover.range = se recuperó { $date_range }

journal.window.transcripts.stretch.removed.erase.date = se borra el { $erase_date }, a menos que lo recuperes.

journal.window.transcripts.stretch.removed.range.and.date = se fue con todo lo de { $date_range }, y solo vuelve junto con todo eso. se borra el { $erase_date }, a menos que lo recuperes.

journal.window.transcripts.tab.names = nombres

journal.window.transcripts.tab.screen.text = texto de pantalla

journal.window.transcripts.tab.signals = señales

journal.window.transcripts.tab.transcript = transcripción

journal.window.transcripts.text.loading.body.context = cargando contexto del cuerpo…

journal.window.transcripts.text.loading.screen.entries = cargando entradas de pantalla…

journal.window.transcripts.text.loading.segment = cargando segmento…

journal.window.transcripts.text.loading.this.day = cargando este día…

journal.window.transcripts.timeline.day.range = línea de tiempo del día ({ $start_time }–{ $end_time })

journal.window.transcripts.timeline.detail.range = línea de tiempo del detalle ({ $start_time }–{ $end_time })

journal.window.transcripts.timeline.duration.minute =
    { $quantity ->
        [one] { $minutes } min
        [many] { $minutes } min
       *[other] { $minutes } min
    }

journal.window.transcripts.timeline.midnight = medianoche

journal.window.transcripts.timeline.segment.accessible = segmento { $details }

journal.window.transcripts.timeline.segment.start = segmento{ $slot1 }{ $time }

journal.window.transcripts.tooltip.close.esc = cerrar (esc)

journal.window.transcripts.tooltip.delete.segment = eliminar segmento

journal.window.transcripts.tooltip.drag.to.adjust.end = arrastrar para ajustar el fin

journal.window.transcripts.tooltip.drag.to.adjust.start = arrastrar para ajustar el inicio

journal.window.transcripts.tooltip.next.frame.right.arrow = fotograma siguiente (flecha derecha)

journal.window.transcripts.tooltip.previous.frame.left.arrow = fotograma anterior (flecha izquierda)

journal.window.transcripts.trash.copy.back.back = atrás

journal.window.transcripts.trash.copy.busy.this.is.already.being.moved.or = esto ya se está moviendo o borrando. recarga en un minuto para revisar.

journal.window.transcripts.trash.copy.cancel.cancel = cancelar

journal.window.transcripts.trash.copy.delete.delete = eliminar

journal.window.transcripts.trash.copy.done.done = listo

journal.window.transcripts.trash.copy.eraseimporting.this.wasn.t.erased.because.it = esto no se borró porque todavía se está importando. vuelve a intentarlo cuando termine.

journal.window.transcripts.trash.copy.eraseother.this.wasn.t.erased.try.again = esto no se borró. vuelve a intentarlo.

journal.window.transcripts.trash.copy.importing.this.wasn.t.moved.because.it = esto no se movió porque todavía se está importando. vuelve a intentarlo cuando termine.

journal.window.transcripts.trash.copy.message.it.may.already.be.back.in = puede que ya haya vuelto a tu diario. recarga para revisar.

journal.window.transcripts.trash.copy.message.it.may.already.be.erased.reload = puede que ya se haya borrado. recarga para revisar.

journal.window.transcripts.trash.copy.message.it.may.already.be.in.the = puede que ya esté en la papelera. recarga para revisar.

journal.window.transcripts.trash.copy.message.it.s.back.in.your.journal = volvió a tu diario.

journal.window.transcripts.trash.copy.message.it.s.in.the.trash.you = está en la papelera. puedes volver a intentarlo desde ahí.

journal.window.transcripts.trash.copy.message.it.s.still.in.the.trash = sigue en la papelera. puedes volver a intentarlo desde ahí.

journal.window.transcripts.trash.copy.message.it.s.still.in.your.journal = sigue en tu diario.

journal.window.transcripts.trash.copy.message.they.may.already.be.back.in = puede que ya hayan vuelto a tu diario. recarga para revisar.

journal.window.transcripts.trash.copy.message.they.may.already.be.erased.reload = puede que ya se hayan borrado. recarga para revisar.

journal.window.transcripts.trash.copy.message.they.may.already.be.in.the = puede que ya estén en la papelera. recarga para revisar.

journal.window.transcripts.trash.copy.message.they.re.back.in.your.journal = volvieron a tu diario.

journal.window.transcripts.trash.copy.message.they.re.still.in.the.trash = siguen en la papelera. puedes volver a intentarlo desde ahí.

journal.window.transcripts.trash.copy.opentrash.open.trash = abrir la papelera

journal.window.transcripts.trash.copy.other.this.wasn.t.moved.try.again = esto no se movió. vuelve a intentarlo.

journal.window.transcripts.trash.copy.recover.recover = recuperar

journal.window.transcripts.trash.copy.select.select = seleccionar

journal.window.transcripts.trash.copy.selectall.select.all = seleccionar todo

journal.window.transcripts.trash.copy.title.couldn.t.erase.those = no se pudieron borrar esos

journal.window.transcripts.trash.copy.title.couldn.t.move.that.to.the = no se pudo mover eso a la papelera

journal.window.transcripts.trash.copy.title.couldn.t.move.those.to.the = no se pudieron mover esos a la papelera

journal.window.transcripts.trash.copy.title.couldn.t.recover.those = no se pudieron recuperar esos

journal.window.transcripts.trash.copy.title.moved.to.the.trash = se movió a la papelera

journal.window.transcripts.trash.copy.title.your.journal.didn.t.answer = tu diario no respondió

journal.window.transcripts.trash.receipt.erase.none.help =
    { $remaining_quantity ->
        [one] búscalo abajo o en la papelera.
        [many] búscalos abajo o en la papelera.
       *[other] búscalos abajo o en la papelera.
    }

journal.window.transcripts.trash.receipt.erase.partial.help =
    { $remaining_quantity ->
        [one] el otro no se borró. búscalo abajo o en la papelera.
        [many] los otros no se borraron. búscalos abajo o en la papelera.
       *[other] los otros no se borraron. búscalos abajo o en la papelera.
    }

journal.window.transcripts.trash.receipt.keep.remaining =
    { $remaining_quantity ->
        [one] { $remaining } sigue en tu diario, con una marca abajo.
        [many] { $remaining } siguen en tu diario, con una marca abajo.
       *[other] { $remaining } siguen en tu diario, con una marca abajo.
    }

journal.window.transcripts.trash.receipt.marked.remaining =
    { $remaining_quantity ->
        [one] no se pudo mover { $remaining } y sigue en tu diario, con una marca abajo.
        [many] no se pudieron mover { $remaining } y siguen en tu diario, con una marca abajo.
       *[other] no se pudieron mover { $remaining } y siguen en tu diario, con una marca abajo.
    }

journal.window.transcripts.trash.receipt.partial.erase =
    { $processed_quantity ->
        [one] se borró { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        }
        [many] se borraron { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        }
       *[other] se borraron { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        }
    }

journal.window.transcripts.trash.receipt.partial.move =
    { $processed_quantity ->
        [one] se movió { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        } a la papelera
        [many] se movieron { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        } a la papelera
       *[other] se movieron { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        } a la papelera
    }

journal.window.transcripts.trash.receipt.partial.recovery =
    { $processed_quantity ->
        [one] se recuperó { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        }
        [many] se recuperaron { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        }
       *[other] se recuperaron { $processed } de { $segments_quantity ->
            [one] { $segments }
            [many] { $segments }
           *[other] { $segments }
        }
    }

journal.window.transcripts.trash.receipt.recovery.partial.help =
    { $remaining_quantity ->
        [one] el otro sigue en la papelera. puedes volver a intentarlo desde ahí.
        [many] los otros siguen en la papelera. puedes volver a intentarlo desde ahí.
       *[other] los otros siguen en la papelera. puedes volver a intentarlo desde ahí.
    }

journal.window.transcripts.trash.receipt.segment.erase.date = se borra el { $erase_date }, a menos que lo recuperes.

journal.window.transcripts.trash.receipt.segment.erase.failed = no se pudo borrar ese segmento

journal.window.transcripts.trash.receipt.segment.erased = se borró un segmento de tu diario

journal.window.transcripts.trash.receipt.segment.recovered = segmento recuperado

journal.window.transcripts.trash.receipt.segment.recovery.failed = no se pudo recuperar ese segmento

journal.window.transcripts.trash.receipt.segments.erase.date = se borran el { $erase_date }, a menos que los recuperes.

journal.window.transcripts.trash.receipt.segments.erased =
    { $segments_quantity ->
        [one] se borró { $segments } segmento de tu diario
        [many] se borraron { $segments } segmentos de tu diario
       *[other] se borraron { $segments } segmentos de tu diario
    }

journal.window.transcripts.trash.receipt.segments.moved =
    { $segments_quantity ->
        [one] se movió { $segments } segmento a la papelera
        [many] se movieron { $segments } segmentos a la papelera
       *[other] se movieron { $segments } segmentos a la papelera
    }

journal.window.transcripts.trash.receipt.segments.recovered =
    { $segments_quantity ->
        [one] { $segments } segmento recuperado
        [many] { $segments } segmentos recuperados
       *[other] { $segments } segmentos recuperados
    }

journal.window.transcripts.trash.selection.count =
    { $segments_quantity ->
        [one] { $segments } seleccionado
        [many] { $segments } seleccionados
       *[other] { $segments } seleccionados
    }

journal.window.transcripts.trash.selection.range = { $start_time } – { $end_time }

journal.window.transcripts.visible.text.an.earlier.audio.copy.attempt.had = un intento anterior de copiar el audio tuvo un problema.

journal.window.transcripts.visible.text.analysis.errored.for.this.segment = el análisis falló en este segmento

journal.window.transcripts.visible.text.analysis.request.failed = no se pudo completar la solicitud de análisis

journal.window.transcripts.visible.text.audio = audio

journal.window.transcripts.visible.text.audio.may.be.incomplete.for.this = el audio puede estar incompleto en este segmento.

journal.window.transcripts.visible.text.audio.variant2 = audio:

journal.window.transcripts.visible.text.body.context = contexto del cuerpo

journal.window.transcripts.visible.text.body.data.present = hay datos del cuerpo.

journal.window.transcripts.visible.text.day = día

journal.window.transcripts.visible.text.day.rhythm = ritmo del día

journal.window.transcripts.visible.text.detail = detalle

journal.window.transcripts.visible.text.import = importación

journal.window.transcripts.visible.text.loading.screen.entries = cargando entradas de pantalla…

journal.window.transcripts.visible.text.loading.this.day = cargando este día…

journal.window.transcripts.visible.text.location.for.this.day.couldn.t = no se pudo cargar el lugar de este día.

journal.window.transcripts.visible.text.no.body.context.in.this.window = no hay contexto del cuerpo en esta ventana.

journal.window.transcripts.visible.text.nothing.in.this.range = no hay nada en este rango

journal.window.transcripts.visible.text.now = ahora

journal.window.transcripts.visible.text.pages = páginas:

journal.window.transcripts.visible.text.rendering.screen.entries = Generando entradas de pantalla…

journal.window.transcripts.visible.text.screen = pantalla

journal.window.transcripts.visible.text.screen.variant2 = pantalla:

journal.window.transcripts.visible.text.show.fewer.entries = mostrar menos entradas

journal.window.transcripts.visible.text.show.full.output = mostrar la salida completa

journal.window.transcripts.visible.text.show.less.output = mostrar menos de la salida

journal.window.transcripts.visible.text.sleep = sueño

journal.window.transcripts.visible.text.still.working = todavía trabajando…

journal.window.transcripts.visible.text.transcripts = transcripciones

journal.window.transcripts.visible.text.warning.details.unavailable = detalles de la advertencia no disponibles

journal.window.transcripts.visible.text.workout = entrenamiento

journal.window.transcripts.visible.text.your.browser.does.not.support.audio = Tu navegador no admite audio.

journal.window.transcripts.warning.age = hace{ $slot1 }{ $age }

journal.window.transcripts.warning.audio.source = { $detail } fuente: { $source_file }

journal.window.transcripts.warning.detail.label = advertencia

journal.window.transcripts.warning.file.unknown = archivo desconocido

journal.window.transcripts.warning.processing.count =
    { $warnings_quantity ->
        [one] 1 advertencia durante el procesamiento
        [many] { $warnings } advertencias durante el procesamiento
       *[other] { $warnings } advertencias durante el procesamiento
    }

journal.window.transcripts.warning.time.unknown = hora desconocida

journal.window.transcripts.window.explanation.audio.may.be.incomplete.for.this = el audio puede estar incompleto en este segmento.

journal.window.transcripts.window.explanation.body.context.unavailable.for.this.day = contexto del cuerpo no disponible para la ventana de este día.

journal.window.transcripts.window.explanation.body.data.present.for.this.day = hay datos del cuerpo para este día.

journal.window.transcripts.window.explanation.body.during.this.hour = el cuerpo durante esta hora

journal.window.transcripts.window.explanation.body.during.this.segment = el cuerpo durante este segmento

journal.window.transcripts.window.explanation.failed.to.fetch.ranges = No se pudieron obtener los rangos

journal.window.transcripts.window.explanation.no.compact.body.context.in.this = no hay contexto compacto del cuerpo en la ventana de este día.

journal.window.transcripts.window.explanation.photos.and.button.presses = fotos y pulsaciones de botones

journal.window.transcripts.window.explanation.this.segment.holds.location.only = este segmento solo guarda el lugar.
