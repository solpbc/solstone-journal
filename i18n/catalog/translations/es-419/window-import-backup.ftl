journal.window.backup.age.backup.failed.clock = el intento de copia de seguridad falló · { $recorded_at }

journal.window.backup.age.backup.failed.day =
    { $quantity ->
        [one] el intento de copia de seguridad falló hace { $count } día
        [many] el intento de copia de seguridad falló hace { $count } días
       *[other] el intento de copia de seguridad falló hace { $count } días
    }

journal.window.backup.age.backup.failed.hour =
    { $quantity ->
        [one] el intento de copia de seguridad falló hace { $count } hora
        [many] el intento de copia de seguridad falló hace { $count } horas
       *[other] el intento de copia de seguridad falló hace { $count } horas
    }

journal.window.backup.age.backup.failed.minute =
    { $quantity ->
        [one] el intento de copia de seguridad falló hace { $count } minuto
        [many] el intento de copia de seguridad falló hace { $count } minutos
       *[other] el intento de copia de seguridad falló hace { $count } minutos
    }

journal.window.backup.age.backup.failed.month =
    { $quantity ->
        [one] el intento de copia de seguridad falló hace { $count } mes
        [many] el intento de copia de seguridad falló hace { $count } meses
       *[other] el intento de copia de seguridad falló hace { $count } meses
    }

journal.window.backup.age.backup.failed.second =
    { $quantity ->
        [one] el intento de copia de seguridad falló hace { $count } segundo
        [many] el intento de copia de seguridad falló hace { $count } segundos
       *[other] el intento de copia de seguridad falló hace { $count } segundos
    }

journal.window.backup.age.backup.failed.week =
    { $quantity ->
        [one] el intento de copia de seguridad falló hace { $count } semana
        [many] el intento de copia de seguridad falló hace { $count } semanas
       *[other] el intento de copia de seguridad falló hace { $count } semanas
    }

journal.window.backup.age.check.failed.clock = la revisión puntual falló · { $recorded_at }

journal.window.backup.age.check.failed.day =
    { $quantity ->
        [one] la revisión puntual falló hace { $count } día
        [many] la revisión puntual falló hace { $count } días
       *[other] la revisión puntual falló hace { $count } días
    }

journal.window.backup.age.check.failed.hour =
    { $quantity ->
        [one] la revisión puntual falló hace { $count } hora
        [many] la revisión puntual falló hace { $count } horas
       *[other] la revisión puntual falló hace { $count } horas
    }

journal.window.backup.age.check.failed.minute =
    { $quantity ->
        [one] la revisión puntual falló hace { $count } minuto
        [many] la revisión puntual falló hace { $count } minutos
       *[other] la revisión puntual falló hace { $count } minutos
    }

journal.window.backup.age.check.failed.month =
    { $quantity ->
        [one] la revisión puntual falló hace { $count } mes
        [many] la revisión puntual falló hace { $count } meses
       *[other] la revisión puntual falló hace { $count } meses
    }

journal.window.backup.age.check.failed.second =
    { $quantity ->
        [one] la revisión puntual falló hace { $count } segundo
        [many] la revisión puntual falló hace { $count } segundos
       *[other] la revisión puntual falló hace { $count } segundos
    }

journal.window.backup.age.check.failed.week =
    { $quantity ->
        [one] la revisión puntual falló hace { $count } semana
        [many] la revisión puntual falló hace { $count } semanas
       *[other] la revisión puntual falló hace { $count } semanas
    }

journal.window.backup.age.check.skipped.clock = no se pudo hacer la revisión puntual · { $recorded_at }

journal.window.backup.age.check.skipped.day =
    { $quantity ->
        [one] no se pudo hacer la revisión puntual hace { $count } día
        [many] no se pudo hacer la revisión puntual hace { $count } días
       *[other] no se pudo hacer la revisión puntual hace { $count } días
    }

journal.window.backup.age.check.skipped.hour =
    { $quantity ->
        [one] no se pudo hacer la revisión puntual hace { $count } hora
        [many] no se pudo hacer la revisión puntual hace { $count } horas
       *[other] no se pudo hacer la revisión puntual hace { $count } horas
    }

journal.window.backup.age.check.skipped.minute =
    { $quantity ->
        [one] no se pudo hacer la revisión puntual hace { $count } minuto
        [many] no se pudo hacer la revisión puntual hace { $count } minutos
       *[other] no se pudo hacer la revisión puntual hace { $count } minutos
    }

journal.window.backup.age.check.skipped.month =
    { $quantity ->
        [one] no se pudo hacer la revisión puntual hace { $count } mes
        [many] no se pudo hacer la revisión puntual hace { $count } meses
       *[other] no se pudo hacer la revisión puntual hace { $count } meses
    }

journal.window.backup.age.check.skipped.second =
    { $quantity ->
        [one] no se pudo hacer la revisión puntual hace { $count } segundo
        [many] no se pudo hacer la revisión puntual hace { $count } segundos
       *[other] no se pudo hacer la revisión puntual hace { $count } segundos
    }

journal.window.backup.age.check.skipped.week =
    { $quantity ->
        [one] no se pudo hacer la revisión puntual hace { $count } semana
        [many] no se pudo hacer la revisión puntual hace { $count } semanas
       *[other] no se pudo hacer la revisión puntual hace { $count } semanas
    }

journal.window.backup.age.elapsed.clock = { $recorded_at }

journal.window.backup.age.elapsed.day =
    { $quantity ->
        [one] hace { $count } día
        [many] hace { $count } días
       *[other] hace { $count } días
    }

journal.window.backup.age.elapsed.hour =
    { $quantity ->
        [one] hace { $count } hora
        [many] hace { $count } horas
       *[other] hace { $count } horas
    }

journal.window.backup.age.elapsed.minute =
    { $quantity ->
        [one] hace { $count } minuto
        [many] hace { $count } minutos
       *[other] hace { $count } minutos
    }

journal.window.backup.age.elapsed.month =
    { $quantity ->
        [one] hace { $count } mes
        [many] hace { $count } meses
       *[other] hace { $count } meses
    }

journal.window.backup.age.elapsed.second =
    { $quantity ->
        [one] hace { $count } segundo
        [many] hace { $count } segundos
       *[other] hace { $count } segundos
    }

journal.window.backup.age.elapsed.week =
    { $quantity ->
        [one] hace { $count } semana
        [many] hace { $count } semanas
       *[other] hace { $count } semanas
    }

journal.window.backup.capacity.gib = { $amount } { $unit }

journal.window.backup.capacity.mib = { $amount } { $unit }

journal.window.backup.consent.destination.backup.only.days =
    { $days_quantity ->
        [one] 1 día de tu diario ({ $backup_size }) existe solo en esa copia de seguridad.
        [many] { $days } días de tu diario ({ $backup_size }) existen solo en esa copia de seguridad.
       *[other] { $days } días de tu diario ({ $backup_size }) existen solo en esa copia de seguridad.
    }

journal.window.backup.consent.teardown.backup.only.days =
    { $days_quantity ->
        [one] { $days } día de tu diario ({ $backup_size }) existe solo en esta copia de seguridad. eliminar la copia de seguridad lo elimina en todas partes, para siempre.
        [many] { $days } días de tu diario ({ $backup_size }) existen solo en esta copia de seguridad. eliminar la copia de seguridad los elimina en todas partes, para siempre.
       *[other] { $days } días de tu diario ({ $backup_size }) existen solo en esta copia de seguridad. eliminar la copia de seguridad los elimina en todas partes, para siempre.
    }

journal.window.backup.copy.action.labels.backup.now = hacer una copia de seguridad ahora

journal.window.backup.copy.action.labels.cancel = cancelar

journal.window.backup.copy.action.labels.change.destination = cambiar destino

journal.window.backup.copy.action.labels.check.cleanup = revisar de nuevo

journal.window.backup.copy.action.labels.cleanup.blocked = no se pudo confirmar que una tarea del diario se haya detenido. esta tarea tiene que detenerse antes de que las acciones de copia de seguridad puedan continuar.

journal.window.backup.copy.action.labels.cleanup.checking = revisando la disponibilidad de la copia de seguridad

journal.window.backup.copy.action.labels.cleanup.contended = vuelve a revisar en un momento.

journal.window.backup.copy.action.labels.cleanup.waiting = las acciones de copia de seguridad están en espera

journal.window.backup.copy.action.labels.enable = activar la copia de seguridad

journal.window.backup.copy.action.labels.finish.cleanup = terminar de detener la copia de seguridad

journal.window.backup.copy.action.labels.finish.task = terminar de detener la tarea

journal.window.backup.copy.action.labels.finishing.cleanup = deteniendo la copia de seguridad…

journal.window.backup.copy.action.labels.finishing.task = deteniendo la tarea…

journal.window.backup.copy.action.labels.pause = pausar la copia de seguridad

journal.window.backup.copy.action.labels.restore = restaurar

journal.window.backup.copy.action.labels.resume = reanudar la copia de seguridad

journal.window.backup.copy.action.labels.rotate.key = regenerar la clave de recuperación

journal.window.backup.copy.action.labels.save.destination = guardar destino

journal.window.backup.copy.action.labels.save.retention = guardar la retención

journal.window.backup.copy.action.labels.start = empezar

journal.window.backup.copy.action.labels.teardown = desactivar y eliminar la copia de seguridad

journal.window.backup.copy.action.labels.try.again = reintentar

journal.window.backup.copy.action.labels.understand = entiendo

journal.window.backup.copy.action.labels.view.key = ver la clave de recuperación

journal.window.backup.copy.brand.lock = tu diario es siempre privado, solo tuyo.

journal.window.backup.copy.confirm.escape = ver la clave otra vez

journal.window.backup.copy.confirm.prompt = escribe la clave de recuperación que acabas de anotar.

journal.window.backup.copy.destination.edit.checking = revisando el destino…

journal.window.backup.copy.destination.edit.confirm.action = usar el nuevo destino

journal.window.backup.copy.destination.edit.confirm.days.unavailable = ahora no se puede revisar cuántos días de tu diario existen solo en esa copia de seguridad.

journal.window.backup.copy.destination.edit.confirm.lead = tu copia de seguridad actual se queda donde está, intacta. cuando cambies, tu diario no va a hacer una copia de seguridad ahí ni restaurar desde ahí.

journal.window.backup.copy.destination.edit.confirm.title = ¿hacer una copia de seguridad en otro lugar?

journal.window.backup.copy.destination.edit.keep.hint = deja las dos vacías para conservar las credenciales que usas ahora.

journal.window.backup.copy.destination.edit.lead = cambia el repositorio, las credenciales o las dos cosas. tu clave de recuperación sigue igual.

journal.window.backup.copy.destination.edit.no.change = ese ya es tu destino.

journal.window.backup.copy.destination.edit.partial.credentials = escribe las dos, o deja las dos vacías para conservar las credenciales que usas ahora.

journal.window.backup.copy.destination.edit.refusals.auth.failed = el destino rechazó las credenciales, así que no se guardó nada. revísalas y vuelve a intentarlo.

journal.window.backup.copy.destination.edit.refusals.bucket.not.found = estas credenciales no pueden abrir un bucket con ese nombre, así que no se guardó nada. revisa el nombre del bucket en el repositorio.

journal.window.backup.copy.destination.edit.refusals.destination.unchanged = ese ya es tu destino.

journal.window.backup.copy.destination.edit.refusals.locked = la copia de seguridad ahí está ocupada, así que no se guardó nada. vuelve a intentarlo pronto.

journal.window.backup.copy.destination.edit.refusals.repository.key.refused = ya hay una copia de seguridad ahí que tu clave de recuperación no puede abrir, así que no se guardó nada. elige otro destino.

journal.window.backup.copy.destination.edit.refusals.timeout = el destino no respondió a tiempo, así que no se guardó nada. revisa el repositorio y las credenciales, y vuelve a intentarlo.

journal.window.backup.copy.destination.edit.refusals.unreachable = no se pudo abrir el destino, así que no se guardó nada. revisa el repositorio y las credenciales, y vuelve a intentarlo.

journal.window.backup.copy.destination.edit.saved.credentials = guardado. las nuevas credenciales abren tu copia de seguridad.

journal.window.backup.copy.destination.edit.saved.existing = guardado. ya hay una copia de seguridad ahí que tu diario puede abrir, así que usa esa.

journal.window.backup.copy.destination.edit.saved.new = guardado. todavía no había una copia de seguridad ahí, así que tu diario configuró una nueva.

journal.window.backup.copy.destination.field.labels.access.key.id = id de clave de acceso

journal.window.backup.copy.destination.field.labels.b2.application.key = clave de aplicación

journal.window.backup.copy.destination.field.labels.b2.key.id = id de clave

journal.window.backup.copy.destination.field.labels.backend = backend

journal.window.backup.copy.destination.field.labels.repository = repositorio

journal.window.backup.copy.destination.field.labels.secret.access.key = clave de acceso secreta

journal.window.backup.copy.destination.modes.byo.desc = tu bucket, tus credenciales. lo predeterminado.

journal.window.backup.copy.destination.modes.byo.note = { $slot1 } nunca está en la ruta.

journal.window.backup.copy.destination.modes.byo.title = tuyo

journal.window.backup.copy.destination.modes.hosted.cta = configurar la copia de seguridad →

journal.window.backup.copy.destination.modes.hosted.desc = { $slot1 } opera por ti la parte fuera del dispositivo.

journal.window.backup.copy.destination.modes.hosted.note = { $slot1 } solo guarda una copia cifrada que se cifró en tu dispositivo antes de salir.

journal.window.backup.copy.destination.modes.hosted.title = operado por { $slot1 }

journal.window.backup.copy.destination.object.lock.summary = notas de configuración del bucket

journal.window.backup.copy.destination.object.lock.warning = no actives Object Lock en modo Compliance en el bucket. choca con la depuración de la copia de seguridad y la limpieza del bloqueo. si necesitas inmutabilidad, usa el modo Governance.

journal.window.backup.copy.destination.reason.labels.auth.failed = el destino rechazó la clave o las credenciales. revisa la clave de recuperación y los detalles del destino.

journal.window.backup.copy.destination.reason.labels.bucket.not.found = estas credenciales no pueden abrir un bucket con ese nombre. revisa que el repositorio use el nombre del bucket, no el de tu clave de almacenamiento. si el bucket todavía no existe, créalo primero con tu proveedor de almacenamiento.

journal.window.backup.copy.destination.reason.labels.locked = el destino está ocupado. vuelve a intentarlo pronto.

journal.window.backup.copy.destination.reason.labels.repo.exists = el destino está al alcance y ya está configurado.

journal.window.backup.copy.destination.reason.labels.repo.missing = el destino está al alcance y necesita configuración.

journal.window.backup.copy.destination.reason.labels.timeout = el destino tardó demasiado en responder. vuelve a intentarlo pronto.

journal.window.backup.copy.destination.reason.labels.unreachable = no se pudo alcanzar el destino. revisa la ubicación de la copia de seguridad y vuelve a intentarlo.

journal.window.backup.copy.destination.repository.hint = el repositorio de restic para tu bucket, por ejemplo { $slot1 }

journal.window.backup.copy.educate.stakes = restaurar requiere dos cosas: tu clave de recuperación, y los detalles de tu almacenamiento o un inicio de sesión en el portal de servicios. si pierdes la clave, nadie puede restaurar tu copia de seguridad. ni siquiera { $slot1 }.

journal.window.backup.copy.error.intro = empieza por la clave de recuperación. si sigue fallando, revisa los detalles del destino.

journal.window.backup.copy.hosted.location.detail.summary = ubicación exacta del almacenamiento

journal.window.backup.copy.hosted.location.label = almacenamiento que opera { $slot1 }

journal.window.backup.copy.hosted.manage.label = administrar tu copia de seguridad en el portal de servicios →

journal.window.backup.copy.hosted.setup.hint = esto configura la copia de seguridad cifrada, operada por { $slot1 }. termina de activarla en el portal de servicios que se abre y vuelve aquí. tu diario se queda en tu dispositivo; solo la copia cifrada va al almacenamiento que opera { $slot2 }, y { $slot3 } nunca puede leerla.

journal.window.backup.copy.intro.bullets.item1 = cifrada en tu dispositivo antes de salir

journal.window.backup.copy.intro.bullets.item2 = opcional, siempre

journal.window.backup.copy.intro.bullets.item3 = eliminar en cualquier momento

journal.window.backup.copy.intro.optional = tu diario vive en tu dispositivo; la copia de seguridad es opcional.

journal.window.backup.copy.intro.steps = vas a guardar una clave de recuperación y luego eliges dónde vive tu copia de seguridad cifrada. restaurar requiere esa clave y una cosa más: los detalles de tu almacenamiento, o un inicio de sesión en el portal de servicios si { $slot1 } opera tu almacenamiento.

journal.window.backup.copy.intro.subtitle = haz una copia cifrada de tu diario en un lugar seguro. solo tú puedes leerla.

journal.window.backup.copy.intro.title = copia de seguridad cifrada

journal.window.backup.copy.key.clipboard.caveat = copiar pone tu clave de recuperación en el portapapeles. límpialo después de guardarla.

journal.window.backup.copy.key.continue = continuar

journal.window.backup.copy.key.copy.failed = no se pudo copiar tu clave de recuperación. guárdala en un archivo o anótala antes de continuar.

journal.window.backup.copy.key.copy.label = copiar

journal.window.backup.copy.key.pm.caution = guarda tu clave de recuperación solo en un gestor de contraseñas en el que confíes. { $slot1 } no recomienda uno en particular.

journal.window.backup.copy.key.save.failed = no se pudo guardar tu clave de recuperación. cópiala o anótala antes de continuar.

journal.window.backup.copy.key.save.key = guardar la clave de recuperación

journal.window.backup.copy.key.theft.honesty = cualquiera con tu clave de recuperación puede leer todo en tu copia de seguridad. guárdala como una contraseña maestra.

journal.window.backup.copy.management.destructive.action = desactivar y eliminar la copia de seguridad

journal.window.backup.copy.management.destructive.caption = esto elimina toda tu copia de seguridad. no se van a crear copias de seguridad nuevas.

journal.window.backup.copy.management.pause.caption = detiene las copias de seguridad, las revisiones puntuales, la limpieza y el traslado de contenido multimedia hasta que reanudes. tu copia de seguridad, las claves y el historial se quedan como están.

journal.window.backup.copy.management.pause.title = pausar la copia de seguridad

journal.window.backup.copy.management.paused.body = no se ejecuta ninguna copia de seguridad hasta que reanudes. tu copia de seguridad se queda como estaba, y restaurar sigue funcionando.

journal.window.backup.copy.management.paused.hosted.note = pausar no desactiva la copia de seguridad cifrada en el portal de servicios.

journal.window.backup.copy.management.paused.title = la copia de seguridad está en pausa

journal.window.backup.copy.management.retention.hint = cuántas copias recientes conservar en cada intervalo.

journal.window.backup.copy.management.retention.labels.daily = diaria

journal.window.backup.copy.management.retention.labels.hourly = por hora

journal.window.backup.copy.management.retention.labels.monthly = mensual

journal.window.backup.copy.management.retention.labels.weekly = semanal

journal.window.backup.copy.management.rotate.key.caption = esto reemplaza tu clave de recuperación. la clave anterior deja de funcionar, así que cada copia que guardaste queda inservible. anota la nueva antes de salir de esta página.

journal.window.backup.copy.management.rotate.key.summary = reemplazar tu clave de recuperación

journal.window.backup.copy.management.status.labels.backup.attempt.unknown = no se conoce el último intento de copia de seguridad.

journal.window.backup.copy.management.status.labels.backup.history.note = este es un resultado anterior. no confirma qué hay guardado en el destino actual de tu copia de seguridad ni si se ha verificado.

journal.window.backup.copy.management.status.labels.backup.history.unknown = no se conoce la última copia de seguridad exitosa.

journal.window.backup.copy.management.status.labels.destination = dónde vive tu copia de seguridad

journal.window.backup.copy.management.status.labels.disabled = desactivada

journal.window.backup.copy.management.status.labels.enabled = activada

journal.window.backup.copy.management.status.labels.last.backup = último intento de copia de seguridad

journal.window.backup.copy.management.status.labels.last.prune = última limpieza

journal.window.backup.copy.management.status.labels.last.successful.backup = última copia de seguridad exitosa

journal.window.backup.copy.management.status.labels.last.verification = última verificación

journal.window.backup.copy.management.status.labels.not.available = todavía no está disponible

journal.window.backup.copy.management.status.labels.not.yet = todavía no

journal.window.backup.copy.management.status.labels.retention = retención

journal.window.backup.copy.management.status.labels.setup = configurar tu clave de recuperación

journal.window.backup.copy.management.teardown.confirm.prompt = escribe delete para confirmar

journal.window.backup.copy.management.teardown.gate.unavailable.lead = ahora no se puede verificar qué existe solo en esta copia de seguridad. eliminarla puede destruir días de tu diario que no existen en ningún otro lugar.

journal.window.backup.copy.management.teardown.gate.zero.lead = ahora no hay nada que exista solo en esta copia de seguridad. cada día sigue en tu dispositivo.

journal.window.backup.copy.management.teardown.restore.first.action = restaurar todo primero

journal.window.backup.copy.offload.action.error = no se pudo terminar el traslado de contenido multimedia. revisa la configuración inicial de la copia de seguridad y vuelve a intentarlo.

journal.window.backup.copy.offload.actions.check.preparation = revisar la preparación

journal.window.backup.copy.offload.actions.disable = desactivar el traslado de contenido multimedia

journal.window.backup.copy.offload.actions.enable = activar el traslado de contenido multimedia

journal.window.backup.copy.offload.actions.prepare = preparar copias

journal.window.backup.copy.offload.actions.restore.day = restaurar este día

journal.window.backup.copy.offload.actions.review.originals = revisar los originales

journal.window.backup.copy.offload.actions.save = guardar límites

journal.window.backup.copy.offload.backup.only.label = conservado solo en la copia de seguridad

journal.window.backup.copy.offload.disable.note = esto se detiene. los días que ya están en tu copia de seguridad se quedan ahí, protegidos y restaurables.

journal.window.backup.copy.offload.enable.hint = elige cuánto contenido multimedia más antiguo puede salir de esta computadora después de la verificación de la copia de seguridad.

journal.window.backup.copy.offload.invalid.limits = escribe un número positivo para cada límite y luego vuelve a guardar.

journal.window.backup.copy.offload.labels.budget.gb = presupuesto de contenido multimedia original

journal.window.backup.copy.offload.labels.budget.short = presupuesto

journal.window.backup.copy.offload.labels.days = días trasladados

journal.window.backup.copy.offload.labels.device.free = espacio libre

journal.window.backup.copy.offload.labels.device.total = espacio total

journal.window.backup.copy.offload.labels.eligible = listo para salir de esta computadora

journal.window.backup.copy.offload.labels.floor.gb = mínimo de espacio libre

journal.window.backup.copy.offload.labels.floor.short = mínimo

journal.window.backup.copy.offload.labels.known = contado hasta ahora

journal.window.backup.copy.offload.labels.last.completed = última finalización

journal.window.backup.copy.offload.labels.last.offload = último traslado

journal.window.backup.copy.offload.labels.last.restore = última restauración

journal.window.backup.copy.offload.labels.last.verify = última verificación

journal.window.backup.copy.offload.labels.projected = en esta computadora después de quitarlo

journal.window.backup.copy.offload.labels.proposed = propuesto para salir de esta computadora

journal.window.backup.copy.offload.labels.raw.media = en esta computadora

journal.window.backup.copy.offload.labels.under.one.mib = menos de 1 { $slot1 }

journal.window.backup.copy.offload.messages.degraded = no se pudo leer parte del registro de lo que hay en tu copia de seguridad. estos días pueden guardar más de lo que se muestra.

journal.window.backup.copy.offload.messages.empty.days = todavía no hay contenido multimedia trasladado.

journal.window.backup.copy.offload.messages.incomplete = algunos totales de contenido multimedia no están disponibles.

journal.window.backup.copy.offload.messages.saved = guardado

journal.window.backup.copy.offload.not.ready = activa la copia de seguridad cifrada y confirma tu clave de recuperación antes de usar el traslado de contenido multimedia.

journal.window.backup.copy.offload.paused.enable.note = reanuda la copia de seguridad para activar el traslado de contenido multimedia.

journal.window.backup.copy.offload.paused.note = el traslado de contenido multimedia está en pausa junto con la copia de seguridad. no sale contenido multimedia de esta computadora hasta que reanudes.

journal.window.backup.copy.offload.preparation.checking = revisando la preparación…

journal.window.backup.copy.offload.preparation.failed = la preparación necesita atención. mira el último resultado del traslado.

journal.window.backup.copy.offload.preparation.finished = mira el último resultado del traslado y los originales propuestos.

journal.window.backup.copy.offload.preparation.pending = un resultado final no está disponible. revisa de nuevo.

journal.window.backup.copy.offload.preparation.unavailable = ahora no se puede confirmar la preparación. revisa de nuevo antes de empezar otra.

journal.window.backup.copy.offload.prepare.hint = prepara copias cifradas para revisar. elige qué originales quitar en la página de originales.

journal.window.backup.copy.offload.reason.unavailable = la razón no está disponible aquí.

journal.window.backup.copy.offload.restore.reason.labels.archive.unbound = el registro de la copia de seguridad no identifica el paquete necesario para restaurar estos archivos.

journal.window.backup.copy.offload.restore.reason.labels.auth.failed = la copia de seguridad cifrada rechazó la clave de recuperación o las credenciales.

journal.window.backup.copy.offload.restore.reason.labels.backup.not.ready = la copia de seguridad cifrada no está lista para restaurar contenido multimedia.

journal.window.backup.copy.offload.restore.reason.labels.failed = la restauración del contenido multimedia no se pudo terminar.

journal.window.backup.copy.offload.restore.reason.labels.insufficient.free.space = esta computadora necesita más espacio libre antes de restaurar el contenido multimedia.

journal.window.backup.copy.offload.restore.reason.labels.ledger.degraded = no se pudo leer parte del registro de lo que hay en tu copia de seguridad, así que no se puede confiar en que una restauración esté completa. vuelve a intentarlo después de que se ejecute la siguiente copia de seguridad.

journal.window.backup.copy.offload.restore.reason.labels.legacy.read.failed = no se pudo leer la copia de seguridad del contenido multimedia trasladado por una versión anterior de tu diario, así que no se restauró.

journal.window.backup.copy.offload.restore.reason.labels.legacy.review.held = no se pudo revisar contra tu copia de seguridad el contenido multimedia trasladado por una versión anterior de tu diario, así que no se restauró.

journal.window.backup.copy.offload.restore.reason.labels.legacy.verification.failed = la copia de seguridad del contenido multimedia trasladado por una versión anterior de tu diario no coincidió con su registro, así que no se restauró.

journal.window.backup.copy.offload.restore.reason.labels.locked = la restauración del contenido multimedia está en espera de que termine el mantenimiento de la copia de seguridad.

journal.window.backup.copy.offload.restore.reason.labels.missing.file.after.restore = la restauración del contenido multimedia terminó, pero todavía faltaba un archivo.

journal.window.backup.copy.offload.restore.reason.labels.nothing.to.restore = no hay nada que restaurar de ese día.

journal.window.backup.copy.offload.restore.reason.labels.repo.missing = la copia de seguridad cifrada no pudo encontrar el repositorio.

journal.window.backup.copy.offload.restore.reason.labels.restore.conflict = ya hay un archivo distinto donde corresponde parte de este contenido multimedia, así que se conservó el archivo que ya estaba y no se restauró la copia que está en la copia de seguridad.

journal.window.backup.copy.offload.restore.reason.labels.restore.held = restaurar los archivos trasladados no está disponible temporalmente.

journal.window.backup.copy.offload.restore.reason.labels.restore.not.published = esta vez no se pudo devolver parte del contenido multimedia a tu diario y no se puso nada en su lugar, así que vuelve a intentarlo más tarde.

journal.window.backup.copy.offload.restore.reason.labels.segment.missing = ese día ya no está disponible localmente.

journal.window.backup.copy.offload.restore.reason.labels.timeout = la restauración del contenido multimedia tardó demasiado. vuelve a intentarlo más tarde.

journal.window.backup.copy.offload.restore.reason.labels.verification.failed = el contenido multimedia restaurado no coincidió con la suma de verificación de la copia de seguridad.

journal.window.backup.copy.offload.restore.report.dispositions.already.present = ya está en esta computadora

journal.window.backup.copy.offload.restore.report.dispositions.conflict = se conservó el archivo existente

journal.window.backup.copy.offload.restore.report.dispositions.published = restaurado

journal.window.backup.copy.offload.restore.report.dispositions.unattempted = no se intentó

journal.window.backup.copy.offload.restore.report.dispositions.unverified = resultado no confirmado

journal.window.backup.copy.offload.restore.report.files = resultados de archivos

journal.window.backup.copy.offload.restore.report.held = todavía necesita atención

journal.window.backup.copy.offload.result.labels.error = falló

journal.window.backup.copy.offload.result.labels.ok = completado

journal.window.backup.copy.offload.result.labels.partial = completado en parte

journal.window.backup.copy.offload.result.labels.skipped = no se ejecutó

journal.window.backup.copy.offload.result.labels.stalled = no se pudo terminar

journal.window.backup.copy.offload.result.labels.unknown = resultado no disponible

journal.window.backup.copy.offload.stakes = tras esto, tu copia de seguridad guarda la única copia de tus días más antiguos. si pierdes tu clave de recuperación, nadie puede recuperarlos. ni siquiera { $slot1 }.

journal.window.backup.copy.offload.stall.reason.labels.archive.failed = el traslado de contenido multimedia no pudo agregar contenido multimedia más antiguo a la copia de seguridad cifrada.

journal.window.backup.copy.offload.stall.reason.labels.backup.failing = la copia de seguridad cifrada necesita una copia reciente en buen estado antes de que el traslado de contenido multimedia pueda ejecutarse.

journal.window.backup.copy.offload.stall.reason.labels.backup.not.ready = la copia de seguridad cifrada tiene que terminar la configuración inicial antes de que el traslado de contenido multimedia pueda ejecutarse.

journal.window.backup.copy.offload.stall.reason.labels.confirm.failed = el traslado de contenido multimedia no pudo verificar el contenido multimedia de la copia de seguridad.

journal.window.backup.copy.offload.stall.reason.labels.confirm.tool.failed = el traslado de contenido multimedia no pudo ejecutar la herramienta de verificación.

journal.window.backup.copy.offload.stall.reason.labels.locked = el traslado de contenido multimedia está en espera de que termine el mantenimiento de la copia de seguridad.

journal.window.backup.copy.offload.stall.reason.labels.unexpected.error = el traslado de contenido multimedia se detuvo inesperadamente. vuelve a intentarlo después de que se ejecute el mantenimiento de la copia de seguridad.

journal.window.backup.copy.offload.stall.reason.labels.verification.failed = falló la verificación de la copia de seguridad. el traslado de contenido multimedia va a esperar una verificación en buen estado.

journal.window.backup.copy.offload.stall.reason.labels.verification.missing = la verificación de la copia de seguridad tiene que ejecutarse antes de que el traslado de contenido multimedia pueda empezar.

journal.window.backup.copy.offload.stall.reason.labels.verification.overdue = la verificación de la copia de seguridad está atrasada. el traslado de contenido multimedia va a esperar una verificación nueva.

journal.window.backup.copy.offload.stalled.lead = no se pudo terminar el traslado.

journal.window.backup.copy.offload.title = traslado de contenido multimedia

journal.window.backup.copy.offload.unavailable.lead = ahora no se puede leer el estado del traslado.

journal.window.backup.copy.offload.unsupported = el traslado de contenido multimedia no está disponible en este dispositivo.

journal.window.backup.copy.offload.verification.integrity.failed = la copia de la copia de seguridad no pasó su revisión de integridad y puede estar dañada.

journal.window.backup.copy.operation.reason.labels.auth.failed = esa clave de recuperación no desbloqueó la copia de seguridad. revisa primero la clave y luego los detalles del destino.

journal.window.backup.copy.operation.reason.labels.backup.busy = otra tarea de copia de seguridad ya está en curso. vuelve a intentarlo en un momento.

journal.window.backup.copy.operation.reason.labels.backup.not.confirmed = confirma tu clave de recuperación antes de activar la copia de seguridad.

journal.window.backup.copy.operation.reason.labels.backup.only.held = tu copia de seguridad guarda la única copia de parte de tu contenido multimedia, así que no se hicieron cambios en la copia de seguridad. restaura todo primero y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.backup.only.unplaced = tu copia de seguridad guarda la única copia de parte del contenido multimedia más antiguo que no se puede asociar con una grabación en esta computadora, así que no se puede restaurar y no se hicieron cambios en la copia de seguridad. si tu diario se movió o se copió, trae de vuelta sus días faltantes y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.backup.operation.failed = no se pudo terminar esa acción de copia de seguridad. revisa la clave de recuperación y el destino, y vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.backup.paused = la copia de seguridad está en pausa. reanúdala y vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.backup.unavailable = no se pudo alcanzar el servicio de copia de seguridad. inícialo y vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.body.rebuild.failed = se restauró tu diario, pero no se pudo reconstruir tu historial del cuerpo a partir de él. la restauración no se finalizó.

journal.window.backup.copy.operation.reason.labels.broker.error = la copia de seguridad cifrada no devolvió configuración utilizable. vuelve a intentarlo pronto.

journal.window.backup.copy.operation.reason.labels.broker.unreachable = no se pudo alcanzar la copia de seguridad cifrada. revisa tu conexión y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.cleanup.pending = no se pudo confirmar que la tarea de copia de seguridad se haya detenido. otras acciones de copia de seguridad están en espera.

journal.window.backup.copy.operation.reason.labels.destination.unsupported = la configuración de tu copia de seguridad está incompleta, así que tu diario no puede conectar con ella, y no se hicieron cambios en la copia de seguridad.

journal.window.backup.copy.operation.reason.labels.expired = la aprobación tardó demasiado. vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.failed = no se pudo terminar la acción de copia de seguridad. revisa la clave de recuperación y el destino, luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.hosted.entitlement.inactive = activa la copia de seguridad cifrada en el portal de servicios y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.incomplete = la acción de copia de seguridad no terminó. puedes volver a intentarlo.

journal.window.backup.copy.operation.reason.labels.integrity.failed = se restauró tu diario en esta computadora, pero la copia de seguridad no pasó su revisión de integridad y puede estar dañada.

journal.window.backup.copy.operation.reason.labels.integrity.unverified = se restauró tu diario en esta computadora, pero no se pudo hacer la revisión de integridad (la copia de seguridad estaba ocupada o se agotó el tiempo), así que la copia de seguridad está sin verificar.

journal.window.backup.copy.operation.reason.labels.invalid.config.value = usa números enteros no negativos y luego vuelve a guardar.

journal.window.backup.copy.operation.reason.labels.invalid.key = esa clave de recuperación no abrió la copia de seguridad. vuelve a escribir la clave de tu copia guardada.

journal.window.backup.copy.operation.reason.labels.invalid.operation.for.state = termina el paso actual de la configuración inicial de la copia de seguridad y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.invalid.request.value = revisa los detalles del destino y vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.locked = el destino está ocupado. vuelve a intentarlo pronto.

journal.window.backup.copy.operation.reason.labels.malformed = no se pudo leer la respuesta. actualiza tu diario y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.missing.required.field = completa los campos obligatorios y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.network.error = no se pudo alcanzar el portal de servicios. revisa tu conexión y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.offload.record.unavailable = el registro de lo que el traslado de contenido multimedia movió a tu copia de seguridad falta o no se puede leer, así que no se hicieron cambios en la copia de seguridad. si tu diario se movió o se copió, trae de vuelta su carpeta de estado y luego vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.recovery.key.mismatch = eso no coincidió con tu clave de recuperación. vuelve a escribir la clave de tu copia guardada.

journal.window.backup.copy.operation.reason.labels.repo.missing = no se encontró un repositorio de copia de seguridad en ese destino.

journal.window.backup.copy.operation.reason.labels.repository.unsupported = el repositorio tiene que empezar con { $slot1 } o { $slot2 }, y coincidir con el backend que elegiste (por ejemplo { $slot3 } o { $slot4 }).

journal.window.backup.copy.operation.reason.labels.restore.prepare.expired = la aprobación tardó demasiado. vuelve a intentarlo.

journal.window.backup.copy.operation.reason.labels.storage.unreached = se detuvo la desactivación y eliminación de tu copia de seguridad antes de eliminar nada, así que es seguro volver a intentarlo.

journal.window.backup.copy.operation.reason.labels.teardown.tool.unavailable = falta el programa de copia de seguridad que viene con tu diario, así que no se hicieron cambios en la copia de seguridad.

journal.window.backup.copy.operation.reason.labels.timeout = el destino tardó demasiado en responder. vuelve a intentarlo pronto.

journal.window.backup.copy.phase.labels.cleanup.pending = la copia de seguridad necesita atención

journal.window.backup.copy.phase.labels.degraded = restaurada, pero sin verificar

journal.window.backup.copy.phase.labels.done = listo

journal.window.backup.copy.phase.labels.empty = todavía sin configurar

journal.window.backup.copy.phase.labels.error = no se pudo terminar

journal.window.backup.copy.phase.labels.loading = cargando…

journal.window.backup.copy.phase.labels.refused = la restauración no estaba disponible

journal.window.backup.copy.phase.labels.restoring = restaurando tu diario…

journal.window.backup.copy.phase.labels.rotating = creando una clave de recuperación nueva…

journal.window.backup.copy.phase.labels.setting.up = configurando tu copia de seguridad…

journal.window.backup.copy.phase.labels.tearing.down = desactivando…

journal.window.backup.copy.restore.expectation = una restauración grande puede tardar un rato. puedes dejar esta página abierta mientras se ejecuta.

journal.window.backup.copy.restore.hosted.byo.desc = almacenamiento que traes tú, al que se llega con credenciales que proporcionas.

journal.window.backup.copy.restore.hosted.choose.lane = ¿dónde está la copia cifrada desde la que estás restaurando?

journal.window.backup.copy.restore.hosted.errors.auth.failed = esa clave de recuperación no abrió la copia de seguridad. revisa la clave y luego intenta iniciar sesión otra vez.

journal.window.backup.copy.restore.hosted.errors.cancelled = se canceló la restauración. puedes volver a intentarlo.

journal.window.backup.copy.restore.hosted.key.label = tu clave de recuperación

journal.window.backup.copy.restore.hosted.key.reassurance = este diario usa tu clave y nunca la envía a { $slot1 }.

journal.window.backup.copy.restore.hosted.key.required = escribe tu clave de recuperación antes de continuar.

journal.window.backup.copy.restore.hosted.lane.intro = escribe tu clave de recuperación, luego inicia sesión en el portal de servicios y confirma la restauración.

journal.window.backup.copy.restore.hosted.lane.title = restaurar desde { $slot1 }

journal.window.backup.copy.restore.hosted.operated.desc = almacenamiento que { $slot1 } opera, al que se llega por el portal de servicios.

journal.window.backup.copy.restore.hosted.popup.preflight.failed = no se abrió la ventana de inicio de sesión. vuelve a intentarlo y revisa si tu navegador la bloqueó.

journal.window.backup.copy.restore.hosted.primary = iniciar sesión para restaurar →

journal.window.backup.copy.restore.hosted.reopen = abrir el inicio de sesión otra vez

journal.window.backup.copy.restore.hosted.state.b = en espera de que apruebes la restauración en el portal de servicios…

journal.window.backup.copy.restore.hosted.state.b.refused.hosted.backup.expired = { $slot1 } eliminó esa copia una vez que pasaron 30 días desde que se detuvo la copia de seguridad cifrada.

journal.window.backup.copy.restore.hosted.state.b.refused.no.hosted.backup = { $slot1 } no guarda una copia cifrada para el acceso que usaste.

journal.window.backup.copy.restore.hosted.status.retryable = algo salió mal. puedes volver a intentarlo.

journal.window.backup.copy.service.name = copia de seguridad cifrada

journal.window.backup.copy.status.last.backup.never.run = no hay una copia de seguridad completada registrada en esta computadora

journal.window.backup.detail.bin.directory = el ejecutable actual no está en un directorio { $bin_directory }

journal.window.backup.detail.bin.parent = el ejecutable actual no tiene directorio padre

journal.window.backup.detail.package.root = no se encontró el directorio raíz del paquete

journal.window.backup.detail.path.line.break = la ruta del diario contiene un salto de línea

journal.window.backup.erasure.paused.count =
    { $things_quantity ->
        [one] 1 elemento borrado espera a que se quite de tus copias de seguridad hasta que reanudes.
        [many] { $things } elementos borrados esperan a que se quiten de tus copias de seguridad hasta que reanudes.
       *[other] { $things } elementos borrados esperan a que se quiten de tus copias de seguridad hasta que reanudes.
    }

journal.window.backup.erasure.running.count =
    { $things_quantity ->
        [one] quitando 1 elemento borrado de tus copias de seguridad…
        [many] quitando { $things } elementos borrados de tus copias de seguridad…
       *[other] quitando { $things } elementos borrados de tus copias de seguridad…
    }

journal.window.backup.link.label.backup.details.in.health = detalles de la copia de seguridad en estado →

journal.window.backup.notice.failure.auth.failed = la clave de tu diario no pudo abrir la copia de seguridad en el destino de tu copia de seguridad. abre la copia de seguridad para revisarla.

journal.window.backup.notice.failure.binding.invalid = se rechazó el enlace de tu diario con la copia de seguridad cifrada. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.failure.broker.unreachable = no se pudo alcanzar la copia de seguridad cifrada. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.failure.hosted.entitlement.inactive = la copia de seguridad cifrada no está activada en el portal de servicios. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.failure.locked = el destino de tu copia de seguridad estaba ocupado con el mantenimiento de la copia de seguridad. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.failure.rclone.unavailable = a la copia de seguridad cifrada le falta una parte que necesita donde se ejecuta tu diario. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.failure.repo.missing = no se encontró una copia de seguridad en el destino de tu copia de seguridad. abre la copia de seguridad para revisarla.

journal.window.backup.notice.failure.repository.unsupported = el repositorio en la configuración de tu copia de seguridad no coincide con su almacenamiento. abre la copia de seguridad para cambiarlo.

journal.window.backup.notice.failure.restic.unavailable = la copia de seguridad cifrada no se puede ejecutar donde se ejecuta tu diario. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.failure.timeout = el último intento se quedó sin tiempo. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.failure.unknown = el último intento se detuvo con un error. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.recovery.complete = el último intento de copia de seguridad terminó. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.recovery.incomplete = el último intento de copia de seguridad terminó, pero no se pudo leer parte de tu diario. abre la copia de seguridad para ver detalles.

journal.window.backup.notice.title.cleared = la copia de seguridad vuelve a funcionar

journal.window.backup.notice.title.raised = la copia de seguridad ha estado fallando

journal.window.backup.offload.capacity.suggested = sugerido: { $suggested_size }

journal.window.backup.offload.days.show.all =
    { $days_quantity ->
        [one] mostrar { $days } día
        [many] mostrar todos los { $days } días
       *[other] mostrar todos los { $days } días
    }

journal.window.backup.offload.restore.capacity.expectation = restaurando { $restore_size } desde tu copia de seguridad. una restauración grande puede tardar un rato.

journal.window.backup.offload.restore.day.accessible.name = restaurar este día: { $day }

journal.window.backup.offload.restore.effects =
    { $files_quantity ->
        [one] archivos restaurados: { $files } · agregados a esta computadora: { $size }
        [many] archivos restaurados: { $files } · agregados a esta computadora: { $size }
       *[other] archivos restaurados: { $files } · agregados a esta computadora: { $size }
    }

journal.window.backup.response.backup.refusal.a.backup.can.t.start.because = una copia de seguridad no puede empezar porque el servicio en segundo plano de tu diario no se está ejecutando. inícialo y luego vuelve a intentarlo.

journal.window.backup.response.backup.refusal.a.backup.couldn.t.be.set = no se pudo configurar una copia de seguridad ahí. no se guardó nada.

journal.window.backup.response.backup.refusal.a.required.field.is.missing = falta un campo obligatorio.

journal.window.backup.response.backup.refusal.backup.can.t.be.turned.on = la copia de seguridad no se puede activar hasta que confirmes tu clave de recuperación.

journal.window.backup.response.backup.refusal.detail.backup.is.disabled = la copia de seguridad está deshabilitada

journal.window.backup.response.backup.refusal.detail.backup.is.off = la copia de seguridad está desactivada

journal.window.backup.response.backup.refusal.detail.backup.is.paused = la copia de seguridad está en pausa

journal.window.backup.response.backup.refusal.detail.backup.keys.are.missing = faltan las claves de la copia de seguridad

journal.window.backup.response.backup.refusal.detail.configure.a.destination.first = configura un destino primero

journal.window.backup.response.backup.refusal.detail.hosted.backup.has.no.destination.to = la copia de seguridad alojada no tiene destino que cambiar

journal.window.backup.response.backup.refusal.detail.invalid.region = región no válida

journal.window.backup.response.backup.refusal.detail.media.offload.is.not.available.on = el traslado de contenido multimedia no está disponible en este dispositivo

journal.window.backup.response.backup.refusal.detail.no.recovery.key.yet = todavía no hay clave de recuperación

journal.window.backup.response.backup.refusal.detail.nothing.was.saved = no se guardó nada

journal.window.backup.response.backup.refusal.detail.own.storage.backup.isn.t.on = la copia de seguridad en almacenamiento propio no está activada

journal.window.backup.response.backup.refusal.detail.repository.must.start.with.the.backend = el repositorio debe empezar con el nombre del backend

journal.window.backup.response.backup.refusal.detail.unsupported.backend = backend no admitido

journal.window.backup.response.backup.refusal.detail.use.the.hosted.restore.prepare.flow = usa el flujo de preparación de la restauración alojada

journal.window.backup.response.backup.refusal.one.of.those.values.couldn.t = no se pudo usar uno de esos valores.

journal.window.backup.response.backup.refusal.restore.couldn.t.start = no se pudo iniciar la restauración.

journal.window.backup.response.backup.refusal.that.action.isn.t.available.in = esa acción no está disponible en el estado actual.

journal.window.backup.response.backup.refusal.that.day.couldn.t.be.used = no se pudo usar ese día.

journal.window.backup.response.backup.refusal.that.didn.t.match.your.recovery = eso no coincidió con tu clave de recuperación.

journal.window.backup.response.backup.refusal.that.request.didn.t.finish = esa solicitud no terminó.

journal.window.backup.response.backup.refusal.that.s.a.different.repository.confirm = ese es un repositorio distinto. confirma el cambio para usarlo.

journal.window.backup.response.backup.refusal.that.s.already.your.destination = ese ya es tu destino.

journal.window.backup.response.backup.refusal.that.setting.couldn.t.be.saved = no se pudo guardar esa configuración porque un valor no era válido.

journal.window.backup.response.backup.refusal.the.restore.from.encrypted.backup.couldn = no se pudo preparar la restauración desde la copia de seguridad cifrada.

journal.window.backup.response.backup.refusal.the.restore.from.encrypted.backup.didn = la restauración desde la copia de seguridad cifrada no terminó.

journal.window.backup.response.backup.refusal.this.journal.already.has.a.hosted = Este diario ya tiene una vinculación de copia de seguridad alojada.

journal.window.backup.response.backup.refusal.this.journal.must.start.its.hosted = Este diario tiene que iniciar primero su traspaso de restauración alojada.

journal.window.backup.response.destination.result.message.a.backup.there.doesn.t.open = una copia de seguridad ahí no se abre con la clave de este diario

journal.window.backup.response.destination.result.message.backup.destination.is.reachable.and.needs = el destino de la copia de seguridad se puede alcanzar y necesita configuración inicial

journal.window.backup.response.destination.result.message.backup.repository.is.reachable = se puede alcanzar el repositorio de copia de seguridad

journal.window.backup.response.destination.result.message.could.not.reach.the.backup.destination = no se pudo alcanzar el destino de la copia de seguridad

journal.window.backup.response.destination.result.message.repository.is.locked.try.again.shortly = el repositorio está bloqueado; vuelve a intentarlo pronto

journal.window.backup.response.destination.result.message.repository.password.was.rejected = se rechazó la contraseña del repositorio

journal.window.backup.response.destination.result.message.the.destination.rejected.the.credentials = el destino rechazó las credenciales

journal.window.backup.response.destination.result.message.these.credentials.can.t.open.a = estas credenciales no pueden abrir un bucket con ese nombre

journal.window.backup.response.detail.missing.field = falta{ $slot1 }{ $name }

journal.window.backup.response.detail.missing.request.body = falta el cuerpo de la solicitud

journal.window.backup.response.hosted.restore.handoff.refusal.detail.a.hosted.restore.handoff.is.already = Ya se está preparando un traspaso de restauración alojada.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.that.hosted.restore.handoff.capability.has = Esa capacidad de traspaso de restauración alojada expiró.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.that.hosted.restore.handoff.capability.is = Esa capacidad de traspaso de restauración alojada no es válida.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.that.hosted.restore.handoff.is.not = Ese traspaso de restauración alojada no está listo para este paso.

journal.window.backup.response.hosted.restore.handoff.refusal.detail.the.hosted.restore.handoff.is.no = El traspaso de restauración alojada ya no es el actual.

journal.window.backup.response.offload.change.partial = no se pudo terminar el cambio del traslado de contenido multimedia; recarga antes de volver a intentarlo.

journal.window.backup.response.pause.message = la copia de seguridad está en pausa. reanúdala primero.

journal.window.backup.response.storage.retention.result.your.b2.bucket.only.hides.deleted = tu bucket de B2 solo oculta los archivos eliminados, a menos que una regla de ciclo de vida para todo el bucket los quite, así que las copias de esta copia de seguridad se quedan ahí hasta que agregues una o las elimines tú.

journal.window.backup.response.storage.retention.result.your.bucket.can.lock.files.against = tu bucket puede bloquear archivos contra la eliminación, así que las copias de esta copia de seguridad se quedan ahí hasta que termine cualquier bloqueo y elimines esas versiones o las propias reglas de tu proveedor las quiten.

journal.window.backup.response.storage.retention.result.your.journal.couldn.t.check.whether = tu diario no pudo revisar si tu almacenamiento conserva los archivos eliminados, así que revisa la configuración de versionado, ciclo de vida y bloqueo de tu bucket para asegurarte de que esta copia de seguridad ya no está.

journal.window.backup.response.storage.retention.result.your.storage.keeps.earlier.versions.of = tu almacenamiento conserva versiones anteriores de los archivos eliminados, así que las copias de esta copia de seguridad se quedan en tu bucket hasta que elimines esas versiones o las propias reglas de tu proveedor las quiten.

journal.window.backup.response.tool.manifest.invalid = parte de la instalación de tu diario está dañada; vuelve a instalar tu diario.

journal.window.backup.response.tool.manifest.missing = parte de la instalación de tu diario falta; vuelve a instalar tu diario.

journal.window.backup.response.tool.member.unreadable = no se pudo leer parte de la instalación de tu diario; si sigue pasando, vuelve a instalar tu diario.

journal.window.backup.response.tool.package.mismatch = la instalación de tu diario no coincide; si acabas de actualizar, reinicia tu diario, y si no, vuelve a instalarlo.

journal.window.backup.response.tool.restart.update = reinicia tu diario para terminar la actualización.

journal.window.backup.response.tool.screen.ocr.unavailable = el OCR de pantalla no está disponible en este objetivo.

journal.window.backup.response.tool.unsafe.path = parte de la instalación de tu diario no es lo que debería ser; vuelve a instalar tu diario.

journal.window.backup.response.tool.unsupported.location = esta copia del diario no se está ejecutando desde un paquete instalado, así que no se pueden revisar sus archivos.

journal.window.backup.response.tool.wrong.product = lo que está instalado aquí no es el diario; vuelve a instalar tu diario.

journal.window.backup.response.tool.wrong.target = tu diario se hizo para otro tipo de computadora; vuelve a instalar la versión para esta.

journal.window.backup.status.activity.active.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: en curso
        [rotate] cambio de la clave de recuperación: en curso
        [stop] desactivación de la copia de seguridad: en curso
        [restore] restauración: en curso
        [prepare] preparación de la copia: en curso
       *[approve] aprobación del paquete: en curso
    }

journal.window.backup.status.activity.active.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: en curso.
        [rotate] cambio de la clave de recuperación: en curso.
        [stop] desactivación de la copia de seguridad: en curso.
        [restore] restauración: en curso.
        [prepare] preparación de la copia: en curso.
       *[approve] aprobación del paquete: en curso.
    }

journal.window.backup.status.activity.cancelled.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: cancelada
        [rotate] cambio de la clave de recuperación: cancelado
        [stop] desactivación de la copia de seguridad: cancelada
        [restore] restauración: cancelada
        [prepare] preparación de la copia: cancelada
       *[approve] aprobación del paquete: cancelada
    }

journal.window.backup.status.activity.cancelled.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: cancelada.
        [rotate] cambio de la clave de recuperación: cancelado.
        [stop] desactivación de la copia de seguridad: cancelada.
        [restore] restauración: cancelada.
        [prepare] preparación de la copia: cancelada.
       *[approve] aprobación del paquete: cancelada.
    }

journal.window.backup.status.activity.cancelling.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: cancelando
        [rotate] cambio de la clave de recuperación: cancelando
        [stop] desactivación de la copia de seguridad: cancelando
        [restore] restauración: cancelando
        [prepare] preparación de la copia: cancelando
       *[approve] aprobación del paquete: cancelando
    }

journal.window.backup.status.activity.cancelling.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: cancelando.
        [rotate] cambio de la clave de recuperación: cancelando.
        [stop] desactivación de la copia de seguridad: cancelando.
        [restore] restauración: cancelando.
        [prepare] preparación de la copia: cancelando.
       *[approve] aprobación del paquete: cancelando.
    }

journal.window.backup.status.activity.cleanup.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: en espera de la limpieza
        [rotate] cambio de la clave de recuperación: en espera de la limpieza
        [stop] desactivación de la copia de seguridad: en espera de la limpieza
        [restore] restauración: en espera de la limpieza
        [prepare] preparación de la copia: en espera de la limpieza
       *[approve] aprobación del paquete: en espera de la limpieza
    }

journal.window.backup.status.activity.cleanup.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: en espera de la limpieza.
        [rotate] cambio de la clave de recuperación: en espera de la limpieza.
        [stop] desactivación de la copia de seguridad: en espera de la limpieza.
        [restore] restauración: en espera de la limpieza.
        [prepare] preparación de la copia: en espera de la limpieza.
       *[approve] aprobación del paquete: en espera de la limpieza.
    }

journal.window.backup.status.activity.completed.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: completada
        [rotate] cambio de la clave de recuperación: completado
        [stop] desactivación de la copia de seguridad: completada
        [restore] restauración: completada
        [prepare] preparación de la copia: completada
       *[approve] aprobación del paquete: completada
    }

journal.window.backup.status.activity.degraded.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: completada con problemas
        [rotate] cambio de la clave de recuperación: completado con problemas
        [stop] desactivación de la copia de seguridad: completada con problemas
        [restore] restauración: completada con problemas
        [prepare] preparación de la copia: completada con problemas
       *[approve] aprobación del paquete: completada con problemas
    }

journal.window.backup.status.activity.degraded.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: completada con problemas.
        [rotate] cambio de la clave de recuperación: completado con problemas.
        [stop] desactivación de la copia de seguridad: completada con problemas.
        [restore] restauración: completada con problemas.
        [prepare] preparación de la copia: completada con problemas.
       *[approve] aprobación del paquete: completada con problemas.
    }

journal.window.backup.status.activity.failed.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: no se pudo terminar
        [rotate] cambio de la clave de recuperación: no se pudo terminar
        [stop] desactivación de la copia de seguridad: no se pudo terminar
        [restore] restauración: no se pudo terminar
        [prepare] preparación de la copia: no se pudo terminar
       *[approve] aprobación del paquete: no se pudo terminar
    }

journal.window.backup.status.activity.failed.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: no se pudo terminar.
        [rotate] cambio de la clave de recuperación: no se pudo terminar.
        [stop] desactivación de la copia de seguridad: no se pudo terminar.
        [restore] restauración: no se pudo terminar.
        [prepare] preparación de la copia: no se pudo terminar.
       *[approve] aprobación del paquete: no se pudo terminar.
    }

journal.window.backup.status.activity.refused.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: no se pudo iniciar
        [rotate] cambio de la clave de recuperación: no se pudo iniciar
        [stop] desactivación de la copia de seguridad: no se pudo iniciar
        [restore] restauración: no se pudo iniciar
        [prepare] preparación de la copia: no se pudo iniciar
       *[approve] aprobación del paquete: no se pudo iniciar
    }

journal.window.backup.status.activity.refused.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: no se pudo iniciar.
        [rotate] cambio de la clave de recuperación: no se pudo iniciar.
        [stop] desactivación de la copia de seguridad: no se pudo iniciar.
        [restore] restauración: no se pudo iniciar.
        [prepare] preparación de la copia: no se pudo iniciar.
       *[approve] aprobación del paquete: no se pudo iniciar.
    }

journal.window.backup.status.activity.subscription.label =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: en espera de terminar en el portal de servicios
        [rotate] cambio de la clave de recuperación: en espera de terminar en el portal de servicios
        [stop] desactivación de la copia de seguridad: en espera de terminar en el portal de servicios
        [restore] restauración: en espera de terminar en el portal de servicios
        [prepare] preparación de la copia: en espera de terminar en el portal de servicios
       *[approve] aprobación del paquete: en espera de terminar en el portal de servicios
    }

journal.window.backup.status.activity.subscription.sentence =
    { $operation_kind ->
        [setup] configuración inicial de la copia de seguridad: en espera de terminar en el portal de servicios.
        [rotate] cambio de la clave de recuperación: en espera de terminar en el portal de servicios.
        [stop] desactivación de la copia de seguridad: en espera de terminar en el portal de servicios.
        [restore] restauración: en espera de terminar en el portal de servicios.
        [prepare] preparación de la copia: en espera de terminar en el portal de servicios.
       *[approve] aprobación del paquete: en espera de terminar en el portal de servicios.
    }

journal.window.backup.status.backup.off = la copia de seguridad está desactivada.

journal.window.backup.status.backup.off.superseded = la copia de seguridad está desactivada. otra copia de tu diario se hizo cargo de la copia de seguridad cifrada.

journal.window.backup.status.check.result.date.known.label =
    { $outcome ->
        [completed] se completó, { $recorded_at }
        [failed] falló, { $recorded_at }
        [never-recorded] no hay un intento completado registrado, { $recorded_at }
       *[could-not-run] no se pudo ejecutar, { $recorded_at }
    }

journal.window.backup.status.check.result.date.known.sentence =
    { $outcome ->
        [completed] se completó, { $recorded_at }.
        [failed] falló, { $recorded_at }.
        [never-recorded] no hay un intento completado registrado, { $recorded_at }.
       *[could-not-run] no se pudo ejecutar, { $recorded_at }.
    }

journal.window.backup.status.check.result.date.unknown.label =
    { $outcome ->
        [completed] se completó, no se conoce la fecha
        [failed] falló, no se conoce la fecha
        [never-recorded] no hay un intento completado registrado, no se conoce la fecha
       *[could-not-run] no se pudo ejecutar, no se conoce la fecha
    }

journal.window.backup.status.check.result.date.unknown.sentence =
    { $outcome ->
        [completed] se completó, no se conoce la fecha.
        [failed] falló, no se conoce la fecha.
        [never-recorded] no hay un intento completado registrado, no se conoce la fecha.
       *[could-not-run] no se pudo ejecutar, no se conoce la fecha.
    }

journal.window.backup.status.fact.last.check = última revisión puntual

journal.window.backup.status.fact.last.success = última copia exitosa

journal.window.backup.status.fact.latest.attempt = último intento

journal.window.backup.status.fact.name = copia de seguridad

journal.window.backup.status.fact.none.reported = nada informado

journal.window.backup.status.fact.off = desactivada

journal.window.backup.status.fact.on = activada

journal.window.backup.status.fact.operation = operación

journal.window.backup.status.last.attempt.completed.date = el último intento de copia de seguridad se completó, { $completed_at }.

journal.window.backup.status.outcome.unknown = no se conoce

journal.window.backup.status.paused.label.date = en pausa por ti, { $paused_at }

journal.window.backup.status.paused.on.date = pausaste la copia de seguridad el { $paused_at }.

journal.window.backup.status.visible.text.technical.details = detalles técnicos

journal.window.backup.status.window.explanation.another.copy.of.your.journal.took = otra copia de tu diario se hizo cargo de la copia de seguridad cifrada.

journal.window.backup.status.window.explanation.backup.is.on.the.latest.attempt = la copia de seguridad está activada. no se conoce el último intento.

journal.window.backup.status.window.explanation.backup.status.is.unavailable.right.now = el estado de la copia de seguridad no está disponible ahora mismo.

journal.window.backup.status.window.explanation.encrypted.backup.can.t.reach.storage = la copia de seguridad cifrada no puede alcanzar el almacenamiento desde esta computadora.

journal.window.backup.status.window.explanation.encrypted.backup.can.t.run.on = la copia de seguridad cifrada no se puede ejecutar en esta computadora.

journal.window.backup.status.window.explanation.encrypted.backup.couldn.t.be.reached = no se pudo alcanzar la copia de seguridad cifrada.

journal.window.backup.status.window.explanation.encrypted.backup.didn.t.return.usable = la copia de seguridad cifrada no devolvió configuración utilizable.

journal.window.backup.status.window.explanation.encrypted.backup.isn.t.turned.on = la copia de seguridad cifrada no está activada en el portal de servicios.

journal.window.backup.status.window.explanation.it.ran.out.of.time = se quedó sin tiempo.

journal.window.backup.status.window.explanation.paused.by.you.date.isn.t = en pausa por ti, no se conoce la fecha

journal.window.backup.status.window.explanation.some.of.your.journal.couldn.t = no se pudo leer parte de tu diario, así que la copia que se guardó está incompleta.

journal.window.backup.status.window.explanation.the.backup.key.couldn.t.open = la clave de la copia de seguridad no pudo abrir el destino.

journal.window.backup.status.window.explanation.the.backup.lock.was.already.held = el bloqueo de la copia de seguridad ya estaba tomado.

journal.window.backup.status.window.explanation.the.backup.tool.stopped.for.a = la herramienta de copia de seguridad se detuvo por un motivo que no dio.

journal.window.backup.status.window.explanation.the.backup.tool.stopped.with.an = la herramienta de copia de seguridad se detuvo con un error.

journal.window.backup.status.window.explanation.the.last.recorded.backup.spot.check = falló la última revisión puntual registrada de la copia de seguridad.

journal.window.backup.status.window.explanation.the.latest.recorded.backup.attempt.failed = falló el último intento de copia de seguridad registrado.

journal.window.backup.status.window.explanation.the.latest.recorded.backup.copy.is = la última copia de seguridad registrada está incompleta.

journal.window.backup.status.window.explanation.the.reason.wasn.t.recorded = el motivo no quedó registrado.

journal.window.backup.status.window.explanation.the.repository.in.your.backup.settings = el repositorio en la configuración de tu copia de seguridad no empieza con { $slot1 } o { $slot2 } para coincidir con su backend, así que la copia de seguridad se detuvo. cambia el destino a un repositorio que empiece con { $slot3 } o { $slot4 }

journal.window.backup.status.window.explanation.these.are.recorded.results.they.don = estos son resultados registrados. no confirman qué hay en tu destino actual ni si se verificó.

journal.window.backup.status.window.explanation.this.computer.can.t.set.up = esta computadora no puede configurar ni restaurar la copia de seguridad cifrada.

journal.window.backup.status.window.explanation.this.computer.s.link.to.encrypted = se rechazó el enlace de esta computadora con la copia de seguridad cifrada.

journal.window.backup.status.window.explanation.you.paused.backup.the.date.isn = pausaste la copia de seguridad. no se conoce la fecha.

journal.window.backup.status.window.explanation.your.backup.destination.couldn.t.be = no se pudo encontrar el destino de tu copia de seguridad.

journal.window.backup.storage.user.backend = tu propio almacenamiento · { $storage_backend }

journal.window.backup.verification.parts.checked =
    { $parts_quantity ->
        [one] se revisó 1 de { $parts } parte de tu copia de seguridad.
        [many] se revisó 1 de { $parts } partes de tu copia de seguridad.
       *[other] se revisó 1 de { $parts } partes de tu copia de seguridad.
    }

journal.window.backup.verification.parts.checked.all = se revisó toda tu copia de seguridad esta semana.

journal.window.backup.visible.text.checking.backup.status = revisando el estado de la copia de seguridad…

journal.window.backup.visible.text.couldn.t.load.backup.settings.reload = no se pudo cargar la configuración de la copia de seguridad. recarga esta página para volver a intentarlo.

journal.window.backup.visible.text.loading.backup.settings = cargando la configuración de la copia de seguridad…

journal.window.backup.visible.text.older.files.moved.off.this.computer = los archivos más antiguos se movieron fuera de esta computadora. tu copia de seguridad completa también incluye archivos que siguen aquí.

journal.window.backup.window.explanation.a.backup.waits.until.this.finishes = una copia de seguridad espera hasta que esto termine. puedes salir de esta página.

journal.window.handoff.sme.already.enabled.detail = solstone.me ya está activado

journal.window.handoff.sme.consent.link.prepare.failed.detail = no se pudo empezar a activar solstone.me. vuelve a intentarlo.

journal.window.handoff.sme.outcome.guidance.expired = la solicitud no se permitió dentro de unos quince minutos. vuelve a activar solstone.me cuando quieras.

journal.window.handoff.sme.outcome.guidance.local.error = esta computadora no pudo terminar de activar solstone.me. vuelve a intentarlo.

journal.window.handoff.sme.outcome.guidance.malformed = services.solstone.app respondió de una forma que esta computadora no esperaba. vuelve a intentarlo en un rato.

journal.window.handoff.sme.outcome.guidance.needs.subscription = termina de activarlo en el portal de servicios, luego vuelve y activa solstone.me otra vez.

journal.window.handoff.sme.outcome.guidance.network.error = no se pudo alcanzar services.solstone.app. revisa la conexión de esta computadora y vuelve a intentarlo.

journal.window.handoff.sme.outcome.guidance.revoked = vuelve a intentarlo cuando quieras.

journal.window.handoff.spl.outcome.guidance.expired = Este enlace de activación ya no está activo. Empieza un flujo de activación nuevo.

journal.window.handoff.spl.outcome.guidance.local.error = No se pudo escribir el estado del servicio local. Revisa los permisos del diario y vuelve a intentarlo.

journal.window.handoff.spl.outcome.guidance.malformed = No se entendió la respuesta del servicio. Actualiza solstone y vuelve a intentarlo.

journal.window.handoff.spl.outcome.guidance.network.error = No se pudo alcanzar el servicio. Revisa el acceso a la red y vuelve a intentarlo.

journal.window.handoff.spl.outcome.guidance.pending = Mantén abierta la página de aprobación mientras termina la solicitud.

journal.window.handoff.spl.outcome.guidance.relay.identity.conflict = este diario ya está configurado con otra identidad. contacta a soporte para restablecerlo y luego vuelve a intentarlo.

journal.window.handoff.spl.outcome.guidance.relay.rotation.unsupported = la clave de seguridad de este diario cambió y todavía no se puede volver a registrar automáticamente. contacta a soporte.

journal.window.handoff.spl.outcome.guidance.relay.unavailable = el servicio de red privada no está disponible ahora. vuelve a intentarlo en un rato.

journal.window.handoff.spl.outcome.guidance.revoked = No se dio el consentimiento. Empieza un flujo de activación nuevo cuando quieras.

journal.window.handoff.spl.private.link.already.enabled.detail = tu red privada ya está activada

journal.window.handoff.spl.private.link.consent.link.prepare.failed.detail = no se pudo preparar el enlace de consentimiento

journal.window.handoff.spl.relay.rejected.with.status = el servidor de retransmisión no pudo terminar de configurar tu red privada (error { $http_status }).

journal.window.handoff.spp.expired = el procesamiento confidencial no está activado. el enlace para activarlo expiró antes de que terminaras en tu navegador. vuelve a activarlo para empezar de nuevo.

journal.window.handoff.spp.local.error = el procesamiento confidencial no está activado. tu diario no pudo guardar la configuración. vuelve a activarlo.

journal.window.handoff.spp.malformed = el procesamiento confidencial no está activado. el portal de servicios envió una respuesta que tu diario no pudo leer. vuelve a activarlo. si sigue pasando, actualiza tu diario.

journal.window.handoff.spp.network.error = el procesamiento confidencial no está activado. tu diario no pudo conectar con el portal de servicios. revisa la conexión de la computadora en la que está tu diario y luego vuelve a activarlo.

journal.window.handoff.spp.not.verified = La atestación de hardware todavía no está verificada. El razonamiento sigue bloqueado hasta que termine la verificación.

journal.window.handoff.spp.pending = termina de activarlo en tu navegador.

journal.window.handoff.spp.revoked = el procesamiento confidencial no está activado. vuelve a activarlo cuando quieras.

journal.window.import.accessible.label.notebook = cuaderno

journal.window.import.accessible.name.choose.file.to.upload = elegir archivo para subir

journal.window.import.accessible.name.close = cerrar

journal.window.import.accessible.name.filter.by.month = filtrar por mes

journal.window.import.answer.yes = sí

journal.window.import.apple.export.too.large = esta exportación es más grande que 4.3 { $slot1 }, lo máximo que sube esta página. en su lugar, ejecuta: { $slot2 }

journal.window.import.apple.words.cancel.cancel = cancelar

journal.window.import.apple.words.keeps.running.this.keeps.running.if.you.close = esto sigue ejecutándose si cierras esta página. un historial largo puede tardar una hora o más.

journal.window.import.apple.words.reading.reading.your.export = leyendo tu exportación…

journal.window.import.body.facet.apple.ask.make.body = ¿crear una faceta del cuerpo para tus entrenamientos, tu sueño y otros datos del cuerpo?

journal.window.import.body.facet.apple.ask.named = ¿guardar tus entrenamientos, tu sueño y otros datos del cuerpo bajo la faceta que nombraste { $facet_name }?

journal.window.import.body.facet.apple.ask.unmute = tu faceta del cuerpo está silenciada. ¿la vuelves a activar para tus entrenamientos, tu sueño y otros datos del cuerpo?

journal.window.import.body.facet.apple.files.named = esto archiva todo lo de esta exportación bajo { $facet_name }, incluso lo que ya está en la página de actividades. la vista previa dice cuántos de esos podrían moverse, como máximo.

journal.window.import.body.facet.apple.no.means = si dices que no, los entrenamientos nuevos y el sueño no se listan en la página de actividades, y los agentes limitados a algunas facetas no los verán. la búsqueda y la app del cuerpo siguen mostrándolos.

journal.window.import.body.facet.apple.none.hint = los entrenamientos y el sueño nuevos de esta importación no aparecen en la página de actividades, y los agentes limitados a algunas facetas no los ven. la búsqueda y la app del cuerpo sí los muestran.

journal.window.import.body.facet.shared.answer.failed = no se pudo guardar tu respuesta. vuelve a intentarlo.

journal.window.import.body.facet.shared.ask.make.body = ¿crear una faceta del cuerpo para tus entrenamientos?

journal.window.import.body.facet.shared.ask.named = ¿archivar tus entrenamientos bajo la faceta que nombraste { $facet_name }?

journal.window.import.body.facet.shared.ask.unmute = tu faceta del cuerpo está silenciada. ¿la vuelves a activar para tus entrenamientos?

journal.window.import.body.facet.shared.files.named = esto archiva cada entrenamiento de esta descarga bajo { $facet_name }, incluso los que ya están en la página de actividades. la vista previa dice cuántos de esos se moverían.

journal.window.import.body.facet.shared.heading = faceta

journal.window.import.body.facet.shared.no = no

journal.window.import.body.facet.shared.no.means = si dices que no, los entrenamientos nuevos no se listan en la página de actividades, no salen personas ni cosas de ellos, y los agentes limitados a algunas facetas no los verán. la búsqueda y la app del cuerpo siguen mostrándolos.

journal.window.import.body.facet.shared.none.hint = los entrenamientos nuevos de esta importación no aparecen en la página de actividades, no salen personas ni cosas de ellos, y los agentes limitados a algunas facetas no los ven. la búsqueda y la app del cuerpo sí los muestran.

journal.window.import.body.facet.shared.none.option = sin faceta

journal.window.import.body.facet.shared.offer.changed = la pregunta de la faceta cambió después de que la viste, así que tu respuesta no se guardó. la tarjeta ahora muestra en qué quedó.

journal.window.import.body.facet.shared.this.import = faceta para esta importación

journal.window.import.body.facet.shared.unanswered = responde primero la pregunta de la faceta y luego previsualiza otra vez.

journal.window.import.body.facet.shared.yes = sí

journal.window.import.body.history.busy = { $reason }{ $slot1 }cuando termine, importa la exportación otra vez.

journal.window.import.body.history.carried = se detuvo antes de terminar. lo continuó tu importación de{ $slot1 }{ $time }.

journal.window.import.body.history.carried.running = se detuvo antes de terminar. lo continuó tu importación de{ $slot1 }{ $time }, que está en curso ahora.

journal.window.import.body.history.followed.done = se detuvo antes de terminar. tu importación de{ $slot1 }{ $time }{ $slot2 }se ejecutó después de ella.

journal.window.import.body.history.followed.running = se detuvo antes de terminar. tu importación de{ $slot1 }{ $time }{ $slot2 }está en curso ahora.

journal.window.import.body.history.partly.saved.many = { $count }{ $slot1 }partes de tu diario no terminaron de guardarse. importar otra vez cualquier exportación de { $slot2 } las vuelve a intentar.

journal.window.import.body.history.partly.saved.one = 1 parte de tu diario no terminó de guardarse. importar otra vez cualquier exportación de { $slot1 } la vuelve a intentar.

journal.window.import.body.history.partly.saved.unknown = algunas partes de tu diario no terminaron de guardarse. importar otra vez cualquier exportación de { $slot1 } las vuelve a intentar.

journal.window.import.body.history.stopped.default = se detuvo antes de terminar. importa la exportación otra vez para terminar.

journal.window.import.body.history.time.label = { $month }{ $slot1 }{ $day },{ $slot2 }{ $clock }

journal.window.import.body.label.didnt.start = no se inició

journal.window.import.body.label.failed = no se trajo

journal.window.import.body.label.partly.saved = guardado en parte

journal.window.import.body.label.saved = guardado

journal.window.import.body.label.stopped = detenido

journal.window.import.body.phase.words.place.sorting.what.it.read = ordenando lo que leyó

journal.window.import.body.phase.words.repair.checking.what.an.earlier.import.left = revisando lo que dejó una importación anterior

journal.window.import.body.phase.words.retry.trying.again.what.didn.t.save = reintentando lo que no se guardó

journal.window.import.body.phase.words.scan.reading.your.export = leyendo tu exportación

journal.window.import.body.phase.words.write.saving = guardando

journal.window.import.body.status.reason.busy = otra importación de { $slot1 } estaba en curso, así que esta no empezó.

journal.window.import.body.status.reason.disk.full = el disco de la computadora donde corre tu diario se llenó antes de terminar.

journal.window.import.body.status.reason.export.incomplete = esta exportación parece incompleta, así que no se guardó nada. expórtala otra vez desde { $slot1 } e importa la copia nueva.

journal.window.import.body.status.reason.export.too.large = esta exportación se descomprime en más de 68.7 { $slot1 }, lo máximo que lee una importación.

journal.window.import.body.status.reason.export.unreadable = no se pudo leer esta exportación. expórtala otra vez desde { $slot1 } e importa la copia nueva.

journal.window.import.body.status.reason.facet = la faceta elegida para esto está silenciada o ya no está, así que no se trajo nada.

journal.window.import.body.status.reason.go.ahead = no tenía tu visto bueno, así que no se trajo nada.

journal.window.import.body.status.reason.no.room = no había espacio en la computadora donde corre tu diario para empezarla, así que no se trajo nada. libera espacio y luego importa la exportación otra vez.

journal.window.import.body.status.reason.not.started = no empezó, así que no se trajo nada.

journal.window.import.body.status.reason.not.this.import = esta importación no contiene esa exportación, así que no se trajo nada.

journal.window.import.body.status.reason.other = se detuvo antes de terminar.

journal.window.import.body.status.reason.permission = al diario no se le permitió abrir la exportación.

journal.window.import.body.status.reason.preview.again = la faceta, la zona horaria o la exportación cambió después de su vista previa, así que no se trajo nada. importa la exportación otra vez desde la tarjeta de { $slot1 }.

journal.window.import.body.status.reason.whole.export = una importación de { $slot1 } siempre lee la exportación completa, así que no se trajo nada.

journal.window.import.body.status.reason.windows.not.saving = no se pudieron guardar demasiadas partes de tu diario seguidas, así que se detuvo.

journal.window.import.body.status.stop.disk = { $reason }{ $slot1 }libera espacio y luego importa la exportación otra vez.

journal.window.import.body.status.stop.resume = { $reason }{ $slot1 }importa la exportación otra vez para terminar.

journal.window.import.body.status.upload.limit.mac = esta exportación es más grande que 4.3 { $slot1 }, lo máximo que sube esta página. en su lugar, en el menú journal de la app del diario, elige abrir terminal de administración y ejecuta: { $slot2 }

journal.window.import.body.status.upload.limit.other = esta exportación es más grande que 4.3 { $slot1 }, lo máximo que sube esta página. en su lugar, desde una terminal en la computadora donde corre tu diario, ejecuta: { $slot2 }

journal.window.import.button.label.check.status = revisar el estado

journal.window.import.button.label.choose.folder = elegir carpeta

journal.window.import.button.label.next = siguiente

journal.window.import.button.label.prev = anterior

journal.window.import.button.label.preview.import = previsualizar la importación

journal.window.import.button.label.validate = validar

journal.window.import.column.heading.entries = entradas

journal.window.import.column.heading.file = archivo

journal.window.import.column.heading.imported.at = importado el

journal.window.import.column.heading.imported.to = importado a

journal.window.import.column.heading.source = fuente

journal.window.import.column.heading.status = estado

journal.window.import.consent.import.removal.erasenow.erase.now = borrar ahora

journal.window.import.consent.import.removal.found.detected.entities.drop.it.too.and = las entidades detectadas también lo sueltan, y las muestras de voz que vengan de esto salen de tus voces conocidas. las entidades que cuidas se quedan, y lo que tu diario escribió sobre ellas puede seguir mencionándolo.

journal.window.import.consent.import.removal.found.detected.entities.drop.it.too.and.variant2 = las entidades detectadas también lo sueltan, y las muestras de voz que vengan de esto salen de tus voces conocidas. las entidades que cuidas no se borran, y lo que tu diario escribió sobre ellas puede seguir mencionándolo.

journal.window.import.consent.import.removal.foundmany.detected.entities.drop.them.too.and = las entidades detectadas también los sueltan, y las muestras de voz que vengan de ellos salen de tus voces conocidas. las entidades que cuidas se quedan, y lo que tu diario escribió sobre ellas puede seguir mencionándolos.

journal.window.import.consent.import.removal.foundmany.detected.entities.drop.them.too.and.variant2 = las entidades detectadas también los sueltan, y las muestras de voz que vengan de ellos salen de tus voces conocidas. las entidades que cuidas no se borran, y lo que tu diario escribió sobre ellas puede seguir mencionándolos.

journal.window.import.consent.import.removal.lead.this.erases.the.import.s.own = esto borra lo propio de la importación:

journal.window.import.consent.import.removal.lead.this.moves.the.import.s.own = esto mueve lo propio de la importación:

journal.window.import.consent.import.removal.movetotrash.move.to.trash = mover a la papelera

journal.window.import.consent.import.removal.ownmany.this.erases.each.import.s.own = esto borra los segmentos propios de cada importación, cualquier archivo original que haya conservado y su fila en tu historial de importación.

journal.window.import.consent.import.removal.ownmany.this.moves.each.import.s.own = esto mueve los segmentos propios de cada importación, cualquier archivo original que haya conservado y su fila en tu historial de importación.

journal.window.import.consent.import.removal.reach.it.leaves.your.journal.now.and = sale de tu diario ahora y no se puede recuperar.

journal.window.import.consent.import.removal.reach.it.leaves.your.journal.now.and.variant2 = sale de tu diario ahora y espera en la papelera. después se borra. puedes recuperarla hasta entonces.

journal.window.import.consent.import.removal.reachmany.they.leave.your.journal.now.and = salen de tu diario ahora y no se pueden recuperar.

journal.window.import.consent.import.removal.reachmany.they.leave.your.journal.now.and.variant2 = salen de tu diario ahora y esperan en la papelera; después se borran. puedes recuperar cada una hasta entonces.

journal.window.import.consent.import.removal.sharedset.some.of.what.they.brought.in = parte de lo que trajeron puede quedarse, porque una importación anterior de tu diario lo trajo primero. queda con esa importación.

journal.window.import.consent.import.removal.sharedset.some.of.what.they.brought.in.variant2 = parte de lo que trajeron puede quedarse y no borrarse, porque una importación anterior de tu diario lo trajo primero. queda con esa importación.

journal.window.import.consent.import.removal.sharedunknown.some.of.what.it.brought.in = parte de lo que trajo puede quedarse, porque una importación anterior de tu diario lo trajo primero. queda con esa importación.

journal.window.import.consent.import.removal.sharedunknown.some.of.what.it.brought.in.variant2 = parte de lo que trajo puede quedarse y no borrarse, porque una importación anterior de tu diario lo trajo primero. queda con esa importación.

journal.window.import.consent.import.removal.stays.activities.made.from.it.drop.it = las actividades hechas a partir de esto lo sueltan ahora, y los relatos de sus días y semanas salen de la búsqueda. tu diario luego vuelve a escribirlos a partir de lo que queda, con el razonamiento que elegiste.

journal.window.import.consent.import.removal.staysbody.activities.summaries.and.entities.made.from = las actividades, los resúmenes y las entidades hechos a partir de esto en otra parte de tu diario no se borran y todavía pueden mencionarlo.

journal.window.import.consent.import.removal.staysbody.activities.summaries.and.entities.made.from.variant2 = las actividades, los resúmenes y las entidades hechos a partir de esto en otra parte de tu diario se quedan donde están y todavía pueden mencionarlo.

journal.window.import.consent.import.removal.staysbodymany.activities.summaries.and.entities.made.from = las actividades, los resúmenes y las entidades hechas a partir de ellos en otra parte de tu diario no se borran y pueden seguir mencionándolos.

journal.window.import.consent.import.removal.staysbodymany.activities.summaries.and.entities.made.from.variant2 = las actividades, los resúmenes y las entidades hechas a partir de ellos en otra parte de tu diario se quedan donde están y pueden seguir mencionándolos.

journal.window.import.consent.import.removal.staysmany.activities.made.from.them.drop.them = las actividades hechas a partir de ellos los sueltan ahora, y los relatos de sus días y semanas salen de la búsqueda. tu diario luego los reescribe desde lo que queda, con el razonamiento que elegiste.

journal.window.import.consent.import.removal.title.delete.this.import = ¿eliminar esta importación?

journal.window.import.consent.import.removal.title.erase.this.import.now = ¿borrar esta importación ahora?

journal.window.import.consent.import.removal.tooknothing.everything.it.brought.in.stays.and = todo lo que trajo se queda y no se borra, porque una importación anterior de tu diario lo trajo primero. queda con esa importación.

journal.window.import.consent.import.removal.tooknothing.everything.it.brought.in.stays.because = todo lo que trajo se queda, porque una importación anterior de tu diario lo trajo primero. queda con esa importación.

journal.window.import.consent.removal.again.apple.health.erase = cada exportación de Apple Health guarda todo tu historial, así que importarla otra vez vuelve a traer lo que esto borra.

journal.window.import.consent.removal.again.apple.health.move = cada exportación de Apple Health guarda todo tu historial, así que importarla otra vez vuelve a traer lo que esto elimina.

journal.window.import.consent.removal.again.oura.erase = una sincronización de Oura deja fuera lo que esto borra. una que pidas, de los días que cubrió, lo vuelve a traer.

journal.window.import.consent.removal.again.oura.move = una sincronización de Oura deja fuera lo que esto elimina. una que pidas, de los días que cubrió, lo vuelve a traer.

journal.window.import.consent.removal.again.strava.erase = importar otra vez una descarga de Strava vuelve a traer los entrenamientos que esto borra.

journal.window.import.consent.removal.again.strava.move = importar otra vez una descarga de Strava vuelve a traer los entrenamientos que esto elimina.

journal.window.import.consent.removal.multiple.erase =
    { $quantity ->
    [one] ¿borrar { $import_count } importación ahora?
    [many] ¿borrar { $import_count } importaciones ahora?
   *[other] ¿borrar { $import_count } importaciones ahora?
    }

journal.window.import.consent.removal.multiple.move =
    { $quantity ->
    [one] ¿eliminar { $import_count } importación?
    [many] ¿eliminar { $import_count } importaciones?
   *[other] ¿eliminar { $import_count } importaciones?
    }

journal.window.import.consent.removal.storage.total = tamaño total: { $storage }

journal.window.import.content.fallback.activity.numbered = Actividad { $number }

journal.window.import.content.fallback.audio = Importación de audio

journal.window.import.content.fallback.conversation.numbered = Conversación { $number }

journal.window.import.content.fallback.conversation.segment = Segmento de conversación

journal.window.import.content.fallback.event.untitled = Evento sin título

journal.window.import.content.fallback.image = Importación de imagen

journal.window.import.content.fallback.note = Nota

journal.window.import.content.month.heading.count =
    { $quantity ->
    [one] { $month } { $year }, { $count } elemento
    [many] { $month } { $year }, { $count } elementos
   *[other] { $month } { $year }, { $count } elementos
    }

journal.window.import.content.speaker.assistant = Asistente

journal.window.import.content.speaker.human = Humano

journal.window.import.content.speaker.unknown = Desconocido

journal.window.import.content.speaker.you = Tú

journal.window.import.content.type.audio = audio

journal.window.import.content.type.conversation = chat

journal.window.import.content.type.document = documento

journal.window.import.content.type.event = evento

journal.window.import.content.type.highlight.group = destacados

journal.window.import.content.type.image = imagen

journal.window.import.content.type.note = nota

journal.window.import.content.type.pdf = pdf

journal.window.import.detail.content.attendees =
    { $quantity ->
    [one] { $count } asistente
    [many] { $count } asistentes
   *[other] { $count } asistentes
    }

journal.window.import.detail.content.duration.minutes = { $minutes } min

journal.window.import.detail.content.highlights =
    { $quantity ->
    [one] { $count } destacado
    [many] { $count } destacados
   *[other] { $count } destacados
    }

journal.window.import.detail.content.messages =
    { $quantity ->
    [one] { $count } mensaje
    [many] { $count } mensajes
   *[other] { $count } mensajes
    }

journal.window.import.detail.content.summary.count =
    { $quantity ->
    [one] { $source_name }, { $count } elemento
    [many] { $source_name }, { $count } elementos
   *[other] { $source_name }, { $count } elementos
    }

journal.window.import.detail.count.entities =
    { $quantity ->
    [one] { $count } entidad
    [many] { $count } entidades
   *[other] { $count } entidades
    }

journal.window.import.detail.count.entries =
    { $quantity ->
    [one] { $count } entrada
    [many] { $count } entradas
   *[other] { $count } entradas
    }

journal.window.import.detail.count.files =
    { $quantity ->
    [one] { $count } archivo
    [many] { $count } archivos
   *[other] { $count } archivos
    }

journal.window.import.detail.duration.completed.day =
    { $quantity ->
    [one] completado en { $count } día
    [many] completado en { $count } días
   *[other] completado en { $count } días
    }

journal.window.import.detail.duration.completed.hour =
    { $quantity ->
    [one] completado en { $count } hora
    [many] completado en { $count } horas
   *[other] completado en { $count } horas
    }

journal.window.import.detail.duration.completed.minute =
    { $quantity ->
    [one] completado en { $count } minuto
    [many] completado en { $count } minutos
   *[other] completado en { $count } minutos
    }

journal.window.import.detail.duration.completed.short = completado en menos de un minuto

journal.window.import.detail.duration.elapsed.day =
    { $quantity ->
    [one] { $count } día
    [many] { $count } días
   *[other] { $count } días
    }

journal.window.import.detail.duration.elapsed.hour =
    { $quantity ->
    [one] { $count } hora
    [many] { $count } horas
   *[other] { $count } horas
    }

journal.window.import.detail.duration.elapsed.minute =
    { $quantity ->
    [one] { $count } minuto
    [many] { $count } minutos
   *[other] { $count } minutos
    }

journal.window.import.detail.duration.elapsed.short = menos de un minuto

journal.window.import.detail.heading.timestamp = importación: { $date } · { $clock_time }

journal.window.import.detail.merge.body.days.hidden = { $quantity ->
        [one] 1 día de datos del cuerpo estaba oculto en el archivo comprimido, así que también está oculto en este diario.
        [many] { $count } días de datos del cuerpo estaban ocultos en el archivo comprimido, así que también están ocultos en este diario.
       *[other] { $count } días de datos del cuerpo estaban ocultos en el archivo comprimido, así que también están ocultos en este diario.
    }

journal.window.import.detail.merge.clock.hours =
    { $quantity ->
    [one] 1 hora de { $source } puede mostrarse a la hora equivocada: se guardó en una zona horaria distinta de la que este diario guarda para { $source }.
    [many] { $count } horas de { $source } pueden mostrarse a la hora equivocada: se guardaron en una zona horaria distinta de la que este diario guarda para { $source }.
   *[other] { $count } horas de { $source } pueden mostrarse a la hora equivocada: se guardaron en una zona horaria distinta de la que este diario guarda para { $source }.
    }

journal.window.import.detail.merge.clock.under.hour = menos de una hora de { $source } puede mostrarse a la hora equivocada: se guardó en una zona horaria distinta de la que este diario guarda para { $source }.

journal.window.import.detail.merge.copied =
    { $quantity ->
    [one] { $count } se copió
    [many] { $count } se copiaron
   *[other] { $count } se copiaron
    }

journal.window.import.detail.merge.created =
    { $quantity ->
    [one] { $count } se creó
    [many] { $count } se crearon
   *[other] { $count } se crearon
    }

journal.window.import.detail.merge.errored =
    { $quantity ->
    [one] { $count } con error
    [many] { $count } con error
   *[other] { $count } con error
    }

journal.window.import.detail.merge.links.left.out =
    { $quantity ->
    [one] { $count } enlace de entidad dejado fuera
    [many] { $count } enlaces de entidades dejados fuera
   *[other] { $count } enlaces de entidades dejados fuera
    }

journal.window.import.detail.merge.merged =
    { $quantity ->
    [one] { $count } se fusionó
    [many] { $count } se fusionaron
   *[other] { $count } se fusionaron
    }

journal.window.import.detail.merge.owner.collision = este diario pertenece a { $journal_owner }, y el diario importado marca a { $archive_owner } como su propietario. el propietario de este diario no cambia; la otra persona entró como una entidad normal.

journal.window.import.detail.merge.owner.collision.staged = este diario pertenece a { $journal_owner }, y el diario importado marca a { $archive_owner } como su propietario. el propietario de este diario no cambia; la otra persona quedó preparada y no se agregó.

journal.window.import.detail.merge.skipped =
    { $quantity ->
    [one] { $count } se omitió
    [many] { $count } se omitieron
   *[other] { $count } se omitieron
    }

journal.window.import.detail.merge.staged =
    { $quantity ->
    [one] { $count } preparado
    [many] { $count } preparado
   *[other] { $count } preparado
    }

journal.window.import.detail.merge.staged.omitted =
    { $quantity ->
    [one] { $count } más preparada
    [many] { $count } más preparadas
   *[other] { $count } más preparadas
    }

journal.window.import.detail.preview.other.units = { $count } { $unit }

journal.window.import.detail.preview.pages.unavailable =
    { $quantity ->
    [one] no se pudo extraer 1 página
    [many] no se pudieron extraer { $count } páginas
   *[other] no se pudieron extraer { $count } páginas
    }

journal.window.import.detail.preview.recordings =
    { $recording_quantity ->
    [one] { $day_quantity ->
        [one] { $recordings } grabación a lo largo de { $days } día
        [many] { $recordings } grabación a lo largo de { $days } días
       *[other] { $recordings } grabación a lo largo de { $days } días
    }
    [many] { $day_quantity ->
        [one] { $recordings } grabaciones a lo largo de { $days } día
        [many] { $recordings } grabaciones a lo largo de { $days } días
       *[other] { $recordings } grabaciones a lo largo de { $days } días
    }
   *[other] { $day_quantity ->
        [one] { $recordings } grabaciones a lo largo de { $days } día
        [many] { $recordings } grabaciones a lo largo de { $days } días
       *[other] { $recordings } grabaciones a lo largo de { $days } días
    }
    }

journal.window.import.detail.provenance.importer = importador de { $source_type }

journal.window.import.detail.provenance.processed = procesado { $finished_at }

journal.window.import.field.label.context = contexto

journal.window.import.field.label.date = fecha

journal.window.import.field.label.keep.originals = conservar originales

journal.window.import.field.label.manifest.filename = nombre del archivo de manifiesto

journal.window.import.field.label.source = fuente

journal.window.import.field.label.time.zone.in.recording.filenames = zona horaria en los nombres de las grabaciones

journal.window.import.field.placeholder.e.g.paste.from.notes.app = p. ej. pegar desde la app de notas

journal.window.import.field.placeholder.e.g.rae.lunch.with.joe = p. ej. rae almuerzo con joe

journal.window.import.field.placeholder.e.g.weekly.planning = p. ej. planificación semanal

journal.window.import.field.placeholder.paste.text.here = pega texto aquí…

journal.window.import.field.placeholder.search.items = buscar elementos…

journal.window.import.guide.apple.health.line1 = obtén tu exportación de { $slot1 }

journal.window.import.guide.apple.health.line11 = antes de importar

journal.window.import.guide.apple.health.line13 = esta importación lee entrenamientos, sueño, tiempo de atención plena, frecuencia cardíaca y otras lecturas, totales diarios como pasos y distancia, y entradas registradas como comida y síntomas. todo lo demás de la exportación queda fuera, y el resumen nombra los otros tipos de registros que omitió. se conserva en la carpeta de tu diario, así que va adonde se copie esa carpeta, y la copia de seguridad cifrada, si la usas, la incluye. la exportación en sí no se conserva después de la importación.

journal.window.import.guide.apple.health.line15 = importar la misma exportación otra vez solo vuelve a traer lo que eliminaste, y una exportación más nueva también agrega lo nuevo. un día que ocultaste en la página del cuerpo sigue oculto.

journal.window.import.guide.apple.health.line3 = en tu iphone, abre la app { $slot1 }

journal.window.import.guide.apple.health.line4 = toca { $slot1 } y luego tu foto o tus iniciales, arriba a la derecha

journal.window.import.guide.apple.health.line5 = toca { $slot1 } y luego { $slot2 }. un historial largo puede tardar un rato, así que deja la app abierta hasta que termine

journal.window.import.guide.apple.health.line6 = envía { $slot1 } a esta computadora. { $slot2 } a una mac, o guardarlo en Archivos y copiarlo hasta aquí, las dos sirven

journal.window.import.guide.apple.health.line7 = súbelo aquí

journal.window.import.guide.apple.health.line9 = si la exportación termina casi al instante y el archivo es muy pequeño, no funcionó. expórtala otra vez.

journal.window.import.guide.chatgpt.line1 = exporta tu historial de { $slot1 }

journal.window.import.guide.chatgpt.line10 = sube aquí el archivo { $slot1 }

journal.window.import.guide.chatgpt.line3 = abre [{ $slot1 }]({ $slot2 })

journal.window.import.guide.chatgpt.line4 = haz clic en el ícono de tu perfil, en la esquina inferior izquierda

journal.window.import.guide.chatgpt.line5 = selecciona { $slot1 }

journal.window.import.guide.chatgpt.line6 = ve a { $slot1 }

journal.window.import.guide.chatgpt.line7 = haz clic en { $slot1 }

journal.window.import.guide.chatgpt.line8 = vas a recibir un correo electrónico con un enlace de descarga. puede tardar unos minutos

journal.window.import.guide.chatgpt.line9 = descarga el archivo { $slot1 } del correo electrónico

journal.window.import.guide.claude.line1 = exporta tu historial de { $slot1 }

journal.window.import.guide.claude.line10 = sube aquí el archivo { $slot1 }

journal.window.import.guide.claude.line3 = abre [{ $slot1 }]({ $slot2 })

journal.window.import.guide.claude.line4 = haz clic en el ícono de tu perfil, en la esquina inferior izquierda

journal.window.import.guide.claude.line5 = selecciona { $slot1 }

journal.window.import.guide.claude.line6 = selecciona { $slot1 } en la barra lateral y luego baja a la sección { $slot2 }

journal.window.import.guide.claude.line7 = haz clic en { $slot1 }

journal.window.import.guide.claude.line8 = vas a recibir un correo electrónico con un enlace de descarga

journal.window.import.guide.claude.line9 = descarga el archivo { $slot1 } del correo electrónico

journal.window.import.guide.gemini.line1 = exporta tu historial de { $slot1 }

journal.window.import.guide.gemini.line10 = descarga el archivo { $slot1 } cuando esté listo

journal.window.import.guide.gemini.line11 = sube aquí el archivo { $slot1 }

journal.window.import.guide.gemini.line3 = ve a [{ $slot1 }]({ $slot2 })

journal.window.import.guide.gemini.line4 = haz clic en { $slot1 }

journal.window.import.guide.gemini.line5 = baja y selecciona solo { $slot1 }

journal.window.import.guide.gemini.line6 = haz clic en { $slot1 } y luego quita la selección de todo excepto { $slot2 }

journal.window.import.guide.gemini.line7 = haz clic en { $slot1 }, luego baja y haz clic en { $slot2 }

journal.window.import.guide.gemini.line8 = elige { $slot1 }, tipo de archivo { $slot2 }, y el tamaño que prefieras

journal.window.import.guide.gemini.line9 = haz clic en { $slot1 }. puede tardar de minutos a horas, según tu historial

journal.window.import.guide.ics.line1 = { $slot1 }

journal.window.import.guide.ics.line10 = selecciona en la barra lateral el calendario que quieres exportar

journal.window.import.guide.ics.line11 = ve a { $slot1 }

journal.window.import.guide.ics.line12 = guarda el archivo { $slot1 }

journal.window.import.guide.ics.line13 = súbelo aquí

journal.window.import.guide.ics.line15 = { $slot1 }

journal.window.import.guide.ics.line17 = abre [Calendario de { $slot1 }]({ $slot2 })

journal.window.import.guide.ics.line18 = ve a { $slot1 }

journal.window.import.guide.ics.line19 = selecciona { $slot1 }

journal.window.import.guide.ics.line20 = bajo { $slot1 }, selecciona el calendario y elige { $slot2 }

journal.window.import.guide.ics.line21 = haz clic en el enlace { $slot1 } para descargar

journal.window.import.guide.ics.line22 = sube aquí el archivo { $slot1 }

journal.window.import.guide.ics.line3 = abre [Configuración de { $slot1 }]({ $slot2 })

journal.window.import.guide.ics.line4 = haz clic en { $slot1 }. esto descarga un archivo { $slot2 } con archivos { $slot3 } de cada calendario

journal.window.import.guide.ics.line5 = sube aquí el archivo { $slot1 }

journal.window.import.guide.ics.line7 = { $slot1 }

journal.window.import.guide.ics.line9 = abre la app { $slot1 } en tu Mac

journal.window.import.guide.journal.archive.line1 = mover un diario

journal.window.import.guide.journal.archive.line3 = en el diario que quieres mover, abre su pantalla de importación.

journal.window.import.guide.journal.archive.line4 = elige { $slot1 } y selecciona { $slot2 }.

journal.window.import.guide.journal.archive.line5 = trae el { $slot1 } descargado a este diario.

journal.window.import.guide.journal.archive.line6 = sube el { $slot1 } aquí, selecciona { $slot2 } para revisar los días que contiene y luego selecciona { $slot3 } para fusionarlo.

journal.window.import.guide.journal.archive.line8 = si el otro diario está en otra máquina, primero descarga su { $slot1 } desde la pantalla de importación de ese diario y luego súbelo aquí.

journal.window.import.guide.step.number = paso { $step_number }

journal.window.import.guide.strava.line1 = consigue tu descarga de { $slot1 }

journal.window.import.guide.strava.line10 = la descarga puede ser grande, porque guarda tus fotos y más. esta importación abre solo la lista de tus entrenamientos que está adentro, { $slot1 }. si prefieres no subir el resto, descomprímelo y sube solo ese archivo.

journal.window.import.guide.strava.line12 = antes de importar

journal.window.import.guide.strava.line14 = estos entrenamientos se conservan en la carpeta de tu diario, así que van adonde se copie esa carpeta, y la copia de seguridad cifrada, si la usas, los incluye. tu descarga de { $slot1 } en sí no se conserva después de la importación.

journal.window.import.guide.strava.line3 = inicia sesión en strava.com en una computadora. { $slot1 } solo ofrece la descarga en su sitio web, no en su app

journal.window.import.guide.strava.line4 = pasa el cursor sobre tu nombre, arriba a la derecha, y elige { $slot1 }

journal.window.import.guide.strava.line5 = elige { $slot1 }, luego { $slot2 } bajo { $slot3 }

journal.window.import.guide.strava.line6 = elige { $slot1 }

journal.window.import.guide.strava.line7 = { $slot1 } te manda un enlace por correo electrónico. puede tardar unas horas. descarga el archivo { $slot2 }

journal.window.import.guide.strava.line8 = súbelo aquí

journal.window.import.handoff.busy = la app todavía está agregando archivos. vuelve a intentarlo en un momento.

journal.window.import.handoff.cancelled = ningún archivo elegido

journal.window.import.handoff.date = elige una fecha, o déjala en blanco para que tu diario elija.

journal.window.import.handoff.delivered = { $name } llegó a tu diario

journal.window.import.handoff.device = desde { $device }

journal.window.import.handoff.health.expiry = revisa esta exportación en menos de una hora, antes de que tu diario quite la copia en espera.

journal.window.import.handoff.health.review = revisar la importación de Apple Health

journal.window.import.handoff.health.unavailable = no se pudo abrir esta exportación. elígela otra vez para revisarla.

journal.window.import.handoff.notebook = nombre del cuaderno (opcional)

journal.window.import.handoff.origin.android = desde la app de solstone para android

journal.window.import.handoff.origin.browser = de tu navegador

journal.window.import.handoff.origin.cli = desde la línea de comandos

journal.window.import.handoff.origin.desktop = desde una computadora

journal.window.import.handoff.origin.ios = desde la app de solstone para iphone y ipad

journal.window.import.handoff.origin.linux = desde la app de solstone para linux

journal.window.import.handoff.origin.macos = desde la app de solstone para mac

journal.window.import.handoff.origin.mobile = desde un dispositivo móvil

journal.window.import.handoff.origin.windows = desde la app de solstone para windows

journal.window.import.handoff.refused = la app no pudo agregar este archivo. vuelve a intentarlo.

journal.window.import.handoff.sending = enviando { $name }

journal.window.import.handoff.waiting = en espera de enviar { $name }

journal.window.import.history.archive.left.out =
    { $quantity ->
    [one] { $count } miembro quedó afuera
    [many] { $count } miembros quedaron afuera
   *[other] { $count } miembros quedaron afuera
    }

journal.window.import.history.archive.processing =
    { $quantity ->
    [one] { $count } en espera de procesar
    [many] { $count } en espera de procesar
   *[other] { $count } en espera de procesar
    }

journal.window.import.history.archive.rejected =
    { $quantity ->
    [one] { $count } parte rechazada
    [many] { $count } partes rechazadas
   *[other] { $count } partes rechazadas
    }

journal.window.import.history.archive.remaining =
    { $quantity ->
    [one] { $count } por guardar
    [many] { $count } por guardar
   *[other] { $count } por guardar
    }

journal.window.import.history.archive.saved =
    { $quantity ->
    [one] { $count } parte guardada
    [many] { $count } partes guardadas
   *[other] { $count } partes guardadas
    }

journal.window.import.history.archive.unavailable =
    { $quantity ->
    [one] { $count } no disponible
    [many] { $count } no disponibles
   *[other] { $count } no disponibles
    }

journal.window.import.history.count.entries.unknown =
    { $import_quantity ->
    [one] { $imports } importación, —.
    [many] { $imports } importaciones, —.
   *[other] { $imports } importaciones, —.
    }

journal.window.import.history.count.summary =
    { $import_quantity ->
    [one] { $entry_quantity ->
        [one] { $imports } importación, { $entries } entrada.
        [many] { $imports } importación, { $entries } entradas.
       *[other] { $imports } importación, { $entries } entradas.
    }
    [many] { $entry_quantity ->
        [one] { $imports } importaciones, { $entries } entrada.
        [many] { $imports } importaciones, { $entries } entradas.
       *[other] { $imports } importaciones, { $entries } entradas.
    }
   *[other] { $entry_quantity ->
        [one] { $imports } importaciones, { $entries } entrada.
        [many] { $imports } importaciones, { $entries } entradas.
       *[other] { $imports } importaciones, { $entries } entradas.
    }
    }

journal.window.import.history.load.more.count =
    { $total_quantity ->
    [one] { $shown_quantity ->
        [one] cargar más ({ $shown_count } de { $total_count } mostrada)
        [many] cargar más ({ $shown_count } de { $total_count } mostradas)
       *[other] cargar más ({ $shown_count } de { $total_count } mostradas)
    }
    [many] { $shown_quantity ->
        [one] cargar más ({ $shown_count } de { $total_count } mostrada)
        [many] cargar más ({ $shown_count } de { $total_count } mostradas)
       *[other] cargar más ({ $shown_count } de { $total_count } mostradas)
    }
   *[other] { $shown_quantity ->
        [one] cargar más ({ $shown_count } de { $total_count } mostrada)
        [many] cargar más ({ $shown_count } de { $total_count } mostradas)
       *[other] cargar más ({ $shown_count } de { $total_count } mostradas)
    }
    }

journal.window.import.history.stalled.import = estancada: sin actualizaciones en 10 minutos. id de importación: { $import_id }.

journal.window.import.import.delete.copy.back.back = atrás

journal.window.import.import.delete.copy.bodyunknown.the.body.data.it.brought.in = los datos del cuerpo que trajo

journal.window.import.import.delete.copy.busy.this.is.already.being.moved.or = esto ya se está moviendo o borrando. recarga en un minuto para revisar.

journal.window.import.import.delete.copy.cancel.cancel = cancelar

journal.window.import.import.delete.copy.delete.delete = eliminar

journal.window.import.import.delete.copy.done.done = listo

journal.window.import.import.delete.copy.eraseimporting.this.wasn.t.erased.because.it = esto no se borró porque todavía se está importando. vuelve a intentarlo cuando termine.

journal.window.import.import.delete.copy.eraseother.this.wasn.t.erased.try.again = esto no se borró. vuelve a intentarlo.

journal.window.import.import.delete.copy.hideday.a.day.you.hide.on.the = un día que ocultas en la página del cuerpo sigue oculto cuando lo importas o lo sincronizas de nuevo.

journal.window.import.import.delete.copy.importing.this.wasn.t.moved.because.it = esto no se movió porque todavía se está importando. vuelve a intentarlo cuando termine.

journal.window.import.import.delete.copy.message.it.may.already.be.back.in = puede que ya haya vuelto a tu diario. recarga para revisar.

journal.window.import.import.delete.copy.message.it.may.already.be.erased.reload = puede que ya se haya borrado. recarga para revisar.

journal.window.import.import.delete.copy.message.it.may.already.be.in.the = puede que ya esté en la papelera. recarga para revisar.

journal.window.import.import.delete.copy.message.it.s.back.in.your.journal = volvió a tu diario.

journal.window.import.import.delete.copy.message.it.s.in.the.trash.you = está en la papelera. puedes volver a intentarlo desde ahí.

journal.window.import.import.delete.copy.message.it.s.still.in.the.trash = sigue en la papelera. puedes volver a intentarlo desde ahí.

journal.window.import.import.delete.copy.message.it.s.still.in.your.journal = sigue en tu diario.

journal.window.import.import.delete.copy.message.they.may.already.be.back.in = puede que ya hayan vuelto a tu diario. recarga para revisar.

journal.window.import.import.delete.copy.message.they.may.already.be.erased.reload = puede que ya se hayan borrado. recarga para revisar.

journal.window.import.import.delete.copy.message.they.may.already.be.in.the = puede que ya estén en la papelera. recarga para revisar.

journal.window.import.import.delete.copy.message.they.re.back.in.your.journal = volvieron a tu diario.

journal.window.import.import.delete.copy.message.they.re.still.in.the.trash = siguen en la papelera. puedes volver a intentarlo desde ahí.

journal.window.import.import.delete.copy.oneunknown.any.of.its.segments.waiting.in = los segmentos suyos que esperan por su cuenta en la papelera se borran con ella.

journal.window.import.import.delete.copy.opentrash.open.trash = abrir la papelera

journal.window.import.import.delete.copy.originalunknown.any.original.file.it.kept = cualquier archivo original que conservó

journal.window.import.import.delete.copy.other.this.wasn.t.moved.try.again = esto no se movió. vuelve a intentarlo.

journal.window.import.import.delete.copy.recover.recover = recuperar

journal.window.import.import.delete.copy.row.its.row.in.your.import.history = su fila en tu historial de importaciones

journal.window.import.import.delete.copy.segmentsunknown.the.segments.it.brought.in = los segmentos que trajo

journal.window.import.import.delete.copy.select.select = seleccionar

journal.window.import.import.delete.copy.selectall.select.all = seleccionar todo

journal.window.import.import.delete.copy.title.couldn.t.erase.those = no se pudieron borrar esos

journal.window.import.import.delete.copy.title.couldn.t.move.that.to.the = no se pudo mover eso a la papelera

journal.window.import.import.delete.copy.title.couldn.t.move.those.to.the = no se pudieron mover esos a la papelera

journal.window.import.import.delete.copy.title.couldn.t.recover.those = no se pudieron recuperar esos

journal.window.import.import.delete.copy.title.moved.to.the.trash = se movió a la papelera

journal.window.import.import.delete.copy.title.your.journal.didn.t.answer = tu diario no respondió

journal.window.import.import.delete.copy.workoutsunknown.the.workouts.it.brought.in = los entrenamientos que trajo

journal.window.import.import.source.names.audio.archive.audio.archive = paquete de audio

journal.window.import.import.source.names.audio.audio.folder = carpeta de audio

journal.window.import.import.source.names.document.document = documento

journal.window.import.import.source.names.ics.calendar = calendario

journal.window.import.import.source.names.image.image = imagen

journal.window.import.import.source.names.journal.archive.journal = diario

journal.window.import.import.source.names.quick.quick.import = importación rápida

journal.window.import.import.source.names.recording.meeting.audio = audio de reunión

journal.window.import.import.source.names.text.text = texto

journal.window.import.import.view.choose.the.time.zone.in.the = elige la zona horaria en los nombres de las grabaciones, o escribe el nombre del archivo de manifiesto.

journal.window.import.import.view.could.not.read.the.archive.size = no se pudo leer el tamaño del paquete

journal.window.import.import.view.could.not.read.the.import.size = no se pudo leer el tamaño de la importación

journal.window.import.import.view.import.failed = no se pudo importar

journal.window.import.import.view.import.status.unavailable = estado de la importación no disponible

journal.window.import.import.view.import.unconfirmed = importación sin confirmar

journal.window.import.import.view.nothing.new = nada nuevo

journal.window.import.import.view.path.not.found = No se encontró la ruta

journal.window.import.import.view.please.enter.the.path.to.your = Por favor, ingresa la ruta de tu carpeta.

journal.window.import.import.view.please.select.a.file.to.import = Por favor, selecciona un archivo para importar.

journal.window.import.import.view.recordings.saved = grabaciones guardadas

journal.window.import.import.view.recordings.saved.processing.continues = grabaciones guardadas; el procesamiento continúa

journal.window.import.import.view.validation.failed = Falló la validación

journal.window.import.label.source.obsidian = { $slot1 }

journal.window.import.label.source.oura = { $slot1 }

journal.window.import.label.source.plaud = { $slot1 }

journal.window.import.link.label.back.to.imports = ← volver a las importaciones

journal.window.import.link.label.back.to.sources = ← volver a las fuentes

journal.window.import.link.label.browse.what.was.imported = explorar lo que se importó →

journal.window.import.link.label.choose.a.source = elige una fuente →

journal.window.import.link.label.download.this.journal = descargar este diario

journal.window.import.link.label.import.another.source = importar otra fuente

journal.window.import.link.label.see.it.in.the.body.app = verlo en la app del cuerpo →

journal.window.import.link.label.see.when.your.devices.last.added.something = ver cuándo tus dispositivos agregaron algo por última vez →

journal.window.import.link.label.view.imported.content = ver el contenido importado

journal.window.import.option.label.all.months = todos los meses

journal.window.import.option.label.all.sources = todas las fuentes

journal.window.import.option.label.choose.a.time.zone = elige una zona horaria

journal.window.import.owner.desc.import.from.any.source.above = importa desde cualquier fuente de arriba.

journal.window.import.owner.heading.couldn.t.load.your.import.history = no se pudo cargar tu historial de importaciones

journal.window.import.owner.heading.no.imports.match.this.filter = ninguna importación coincide con este filtro

journal.window.import.owner.heading.no.imports.yet = todavía no hay importaciones

journal.window.import.owner.message.check.your.import.history.before.trying = revisa tu historial de importaciones antes de volver a intentarlo.

journal.window.import.owner.title.upload.interrupted = subida interrumpida

journal.window.import.ox.detail.back.to.imports = volver a las importaciones

journal.window.import.ox.detail.checking = revisando qué hay en { $name }…

journal.window.import.ox.detail.context.hint = una nota breve sobre qué es esto, para encontrarlo más fácil después

journal.window.import.ox.detail.context.label = contexto (opcional)

journal.window.import.ox.detail.discard.body = esto quita de tu diario el archivo que no se inició. todavía no se importó nada.

journal.window.import.ox.detail.discard.button = descartar la copia en espera

journal.window.import.ox.detail.discard.cancel = cancelar

journal.window.import.ox.detail.discard.confirm = descartar

journal.window.import.ox.detail.discard.title = ¿descartar esta copia en espera?

journal.window.import.ox.detail.expired.badge = vencida

journal.window.import.ox.detail.expired.body = esta copia en espera se quitó después de una hora sin empezar.

journal.window.import.ox.detail.expired.recovery = si lo agregaste desde una app de solstone, abre la importación ahí para enviarlo de nuevo, o vuelve a las importaciones para elegir el archivo otra vez.

journal.window.import.ox.detail.expiry.note = si sales sin empezar, tu diario quita la copia en espera después de una hora.

journal.window.import.ox.detail.facet.label = ¿en qué faceta deben ir?

journal.window.import.ox.detail.found.heading = lo que tu diario encontró

journal.window.import.ox.detail.keep.originals.hint = activa esto si quieres una copia de los archivos originales en tu diario.

journal.window.import.ox.detail.no.facet = sin faceta

journal.window.import.ox.detail.nothing.new = nada nuevo para traer de este archivo.

journal.window.import.ox.detail.space.available = necesita unos { $need } · { $free } libres donde vive tu diario

journal.window.import.ox.detail.space.insufficient = no hay suficiente espacio libre donde vive tu diario para traer esto. libera espacio ahí y vuelve a intentarlo.

journal.window.import.ox.detail.try.preview.again = reintentar la vista previa

journal.window.import.ox.discard.already.started = la importación ya se inició o ya se procesó

journal.window.import.ox.discard.import.active = la importación está activa ahora

journal.window.import.ox.discard.import.locked = la importación está bloqueada

journal.window.import.ox.discard.journal.busy = el diario está ocupado

journal.window.import.ox.discard.metadata.unverified = no se pueden verificar los metadatos de la importación

journal.window.import.ox.discard.not.waiting = la importación ya no está en espera de empezar

journal.window.import.ox.history.review.link = revisar y confirmar

journal.window.import.ox.status.ready.to.review = disponible para revisar

journal.window.import.ox.tray.attention.badge = necesita atención

journal.window.import.ox.tray.attention.remove = abre la importación en la app de solstone para quitarla.

journal.window.import.ox.tray.attention.retryable = abre la importación en la app de solstone para volver a intentarlo o quitarla.

journal.window.import.ox.tray.completed.summary = completada · { $summary }

journal.window.import.ox.tray.heading = desde este dispositivo

journal.window.import.ox.tray.in.journal = en tu diario

journal.window.import.ox.tray.in.journal.stage = en tu diario · { $stage }

journal.window.import.ox.tray.review.action = revisar y confirmar

journal.window.import.ox.tray.review.detail = se guardó en tu diario. previsualiza y confirma para empezar.

journal.window.import.ox.tray.review.heading = en espera de tu revisión

journal.window.import.ox.tray.sending.device = enviando desde tu { $device }…

journal.window.import.ox.tray.sending.generic = enviando a tu diario…

journal.window.import.ox.tray.view.import = ver la importación

journal.window.import.ox.tray.waiting = en espera en la app de solstone

journal.window.import.preview.apple.daily.totals =
    { $quantity ->
    [one] { $count } total diario
    [many] { $count } totales diarios
   *[other] { $count } totales diarios
    }

journal.window.import.preview.apple.daily.totals.label =
    { $quantity ->
    [one] total diario
    [many] totales diarios
   *[other] totales diarios
    }

journal.window.import.preview.apple.left.out =
    { $quantity ->
    [one] { $count } otro tipo quedó afuera, nombrado en el resumen
    [many] { $count } otros tipos quedaron afuera, nombrados en el resumen
   *[other] { $count } otros tipos quedaron afuera, nombrados en el resumen
    }

journal.window.import.preview.apple.logged =
    { $quantity ->
    [one] { $count } entrada registrada
    [many] { $count } entradas registradas
   *[other] { $count } entradas registradas
    }

journal.window.import.preview.apple.logged.label =
    { $quantity ->
    [one] entrada registrada
    [many] entradas registradas
   *[other] entradas registradas
    }

journal.window.import.preview.apple.mindful =
    { $quantity ->
    [one] { $count } sesión de atención plena
    [many] { $count } sesiones de atención plena
   *[other] { $count } sesiones de atención plena
    }

journal.window.import.preview.apple.mindful.label =
    { $quantity ->
    [one] sesión de atención plena
    [many] sesiones de atención plena
   *[other] sesiones de atención plena
    }

journal.window.import.preview.apple.move.listed =
    { $quantity ->
    [one] decir que sí puede mover hasta { $count } registro que ya está en la página de actividades a la faceta de esta importación
    [many] decir que sí puede mover hasta { $count } registros que ya están en la página de actividades a la faceta de esta importación
   *[other] decir que sí puede mover hasta { $count } registros que ya están en la página de actividades a la faceta de esta importación
    }

journal.window.import.preview.apple.nights =
    { $quantity ->
    [one] { $count } noche compuesta a partir de etapas de sueño
    [many] { $count } noches compuestas a partir de etapas de sueño
   *[other] { $count } noches compuestas a partir de etapas de sueño
    }

journal.window.import.preview.apple.nights.label =
    { $quantity ->
    [one] noche compuesta a partir de etapas de sueño
    [many] noches compuestas a partir de etapas de sueño
   *[other] noches compuestas a partir de etapas de sueño
    }

journal.window.import.preview.apple.readings =
    { $quantity ->
    [one] { $count } lectura
    [many] { $count } lecturas
   *[other] { $count } lecturas
    }

journal.window.import.preview.apple.readings.label =
    { $quantity ->
    [one] lectura
    [many] lecturas
   *[other] lecturas
    }

journal.window.import.preview.apple.workouts =
    { $quantity ->
    [one] { $count } entrenamiento
    [many] { $count } entrenamientos
   *[other] { $count } entrenamientos
    }

journal.window.import.preview.apple.workouts.label =
    { $quantity ->
    [one] entrenamiento
    [many] entrenamientos
   *[other] entrenamientos
    }

journal.window.import.preview.archive.days =
    { $quantity ->
    [one] { $count } día
    [many] { $count } días
   *[other] { $count } días
    }

journal.window.import.preview.archive.left.out =
    { $quantity ->
    [one] { $count } quedó afuera
    [many] { $count } quedaron afuera
   *[other] { $count } quedaron afuera
    }

journal.window.import.preview.archive.recording.bytes =
    { $quantity ->
    [one] { $count } byte de grabación
    [many] { $count } bytes de grabación
   *[other] { $count } bytes de grabación
    }

journal.window.import.preview.archive.saved =
    { $quantity ->
    [one] { $count } parte de audio ya guardada
    [many] { $count } partes de audio ya guardadas
   *[other] { $count } partes de audio ya guardadas
    }

journal.window.import.preview.archive.source.files =
    { $quantity ->
    [one] { $count } archivo fuente
    [many] { $count } archivos fuente
   *[other] { $count } archivos fuente
    }

journal.window.import.preview.date.range = { $first_date } a { $last_date }

journal.window.import.preview.skipped =
    { $quantity ->
    [one] { $count } se omitió
    [many] { $count } se omitieron
   *[other] { $count } se omitieron
    }

journal.window.import.preview.storage.available = necesita unos { $needed } libres, y { $free } está libre

journal.window.import.preview.workouts.move.listed =
    { $quantity ->
    [one] { $count } entrenamiento que ya está en la página de actividades se movería a la faceta de esta importación desde { $sources }
    [many] { $count } entrenamientos que ya están en la página de actividades se moverían a la faceta de esta importación desde { $sources }
   *[other] { $count } entrenamientos que ya están en la página de actividades se moverían a la faceta de esta importación desde { $sources }
    }

journal.window.import.progress.archive.left.out =
    { $quantity ->
    [one] { $count } quedó afuera
    [many] { $count } quedaron afuera
   *[other] { $count } quedaron afuera
    }

journal.window.import.progress.archive.processed =
    { $quantity ->
    [one] se procesó { $count }
    [many] se procesaron { $count }
   *[other] se procesaron { $count }
    }

journal.window.import.progress.archive.processing =
    { $quantity ->
    [one] { $count } en espera del procesamiento
    [many] { $count } en espera del procesamiento
   *[other] { $count } en espera del procesamiento
    }

journal.window.import.progress.archive.remaining =
    { $quantity ->
    [one] { $count } todavía por guardar
    [many] { $count } todavía por guardar
   *[other] { $count } todavía por guardar
    }

journal.window.import.progress.archive.saved =
    { $quantity ->
    [one] { $count } parte de audio guardada
    [many] { $count } partes de audio guardadas
   *[other] { $count } partes de audio guardadas
    }

journal.window.import.progress.body.days = día { $days_done } de { $days_total }

journal.window.import.progress.completed.pages =
    { $quantity ->
    [one] se extrajo parcialmente (1 página no disponible)
    [many] se extrajo parcialmente ({ $count } páginas no disponibles)
   *[other] se extrajo parcialmente ({ $count } páginas no disponibles)
    }

journal.window.import.progress.completed.time = tardó { $elapsed_time }

journal.window.import.progress.count.entities =
    { $quantity ->
    [one] { $count } entidad
    [many] { $count } entidades
   *[other] { $count } entidades
    }

journal.window.import.progress.count.entities.found =
    { $quantity ->
    [one] { $count } entidad encontrada
    [many] { $count } entidades encontradas
   *[other] { $count } entidades encontradas
    }

journal.window.import.progress.count.entries =
    { $quantity ->
    [one] { $count } entrada
    [many] { $count } entradas
   *[other] { $count } entradas
    }

journal.window.import.progress.date.end = hasta { $last_date }

journal.window.import.progress.date.start = desde { $first_date }

journal.window.import.progress.items.ratio =
    { $total_quantity ->
    [one] { $processed }/{ $total } elemento
    [many] { $processed }/{ $total } elementos
   *[other] { $processed }/{ $total } elementos
    }

journal.window.import.progress.reading.source = leyendo { $source }…

journal.window.import.progress.repeat.document = importar otro documento

journal.window.import.progress.repeat.image = importar otra imagen

journal.window.import.progress.stage.saving = Guardando…

journal.window.import.progress.thinking.held =
    { $quantity ->
    [one] { $count } quedó aparte — { $health_link_start }revisar en Health{ $health_link_end }
    [many] { $count } quedaron aparte — { $health_link_start }revisar en Health{ $health_link_end }
   *[other] { $count } quedaron aparte — { $health_link_start }revisar en Health{ $health_link_end }
    }

journal.window.import.progress.thinking.ready =
    { $total_quantity ->
    [one] { $ready } de { $total } grabación lista para encontrar
    [many] { $ready } de { $total } grabaciones listas para encontrar
   *[other] { $ready } de { $total } grabaciones listas para encontrar
    }

journal.window.import.progress.thinking.unreadable =
    { $quantity ->
    [one] { $count } no se pudo revisar
    [many] { $count } no se pudieron revisar
   *[other] { $count } no se pudieron revisar
    }

journal.window.import.progress.thinking.waiting =
    { $quantity ->
    [one] { $count } en espera
    [many] { $count } en espera
   *[other] { $count } en espera
    }

journal.window.import.progress.time.elapsed = { $elapsed_time } transcurrido

journal.window.import.progress.time.hours.minutes = { $hours } h { $minutes } min

journal.window.import.progress.time.minutes.seconds = { $minutes }m { $seconds }s

journal.window.import.progress.time.seconds = { $seconds } s

journal.window.import.progress.time.stage.total = { $stage_time } ({ $total_time } en total)

journal.window.import.removal.date.imported = se importó el { $day }

journal.window.import.removal.items.body.day = datos del cuerpo de { $day }

journal.window.import.removal.items.body.span = datos del cuerpo de { $first_day } a { $last_day }

journal.window.import.removal.items.originals =
    { $quantity ->
    [one] el archivo original que conservó
    [many] los { $original_count } archivos originales que conservó
   *[other] los { $original_count } archivos originales que conservó
    }

journal.window.import.removal.items.segment.day =
    { $quantity ->
    [one] { $segment_count } segmento de { $day }
    [many] { $segment_count } segmentos de { $day }
   *[other] { $segment_count } segmentos de { $day }
    }

journal.window.import.removal.items.segment.span =
    { $quantity ->
    [one] { $segment_count } segmento de { $first_day } a { $last_day }
    [many] { $segment_count } segmentos de { $first_day } a { $last_day }
   *[other] { $segment_count } segmentos de { $first_day } a { $last_day }
    }

journal.window.import.removal.items.workout.day =
    { $quantity ->
    [one] { $workout_count } entrenamiento de { $day }
    [many] { $workout_count } entrenamientos de { $day }
   *[other] { $workout_count } entrenamientos de { $day }
    }

journal.window.import.removal.items.workout.span =
    { $quantity ->
    [one] { $workout_count } entrenamiento de { $first_day } a { $last_day }
    [many] { $workout_count } entrenamientos de { $first_day } a { $last_day }
   *[other] { $workout_count } entrenamientos de { $first_day } a { $last_day }
    }

journal.window.import.removal.receipt.erase.date.many = se borran el { $scheduled_erase_day } a menos que las recuperes.

journal.window.import.removal.receipt.erase.date.one = se borra el { $scheduled_erase_day } a menos que la recuperes.

journal.window.import.removal.receipt.erased.import = importación borrada de tu diario

journal.window.import.removal.receipt.erased.imports =
    { $quantity ->
    [one] se borró { $import_count } importación de tu diario
    [many] se borraron { $import_count } importaciones de tu diario
   *[other] se borraron { $import_count } importaciones de tu diario
    }

journal.window.import.removal.receipt.gone.more =
    { $quantity ->
    [one] { $no_longer_listed } más ya no aparece.
    [many] { $no_longer_listed } más ya no aparecen.
   *[other] { $no_longer_listed } más ya no aparecen.
    }

journal.window.import.removal.receipt.import.erase.failed = no se pudo borrar esa importación

journal.window.import.removal.receipt.import.recover.failed = no se pudo recuperar esa importación

journal.window.import.removal.receipt.marked =
    { $quantity ->
    [one] { $marked_count } no se pudo mover y está marcada abajo.
    [many] { $marked_count } no se pudieron mover y están marcadas abajo.
   *[other] { $marked_count } no se pudieron mover y están marcadas abajo.
    }

journal.window.import.removal.receipt.moved.imports =
    { $quantity ->
    [one] se movió { $import_count } importación a la papelera
    [many] se movieron { $import_count } importaciones a la papelera
   *[other] se movieron { $import_count } importaciones a la papelera
    }

journal.window.import.removal.receipt.none.erased.note =
    { $quantity ->
    [one] búscala abajo o en la papelera.
    [many] búscalas abajo o en la papelera.
   *[other] búscalas abajo o en la papelera.
    }

journal.window.import.removal.receipt.none.gone =
    { $quantity ->
    [one] ya no aparece aquí.
    [many] ya no aparecen aquí.
   *[other] ya no aparecen aquí.
    }

journal.window.import.removal.receipt.none.still =
    { $quantity ->
    [one] { $still_listed } sigue en tu diario, marcada abajo.
    [many] { $still_listed } siguen en tu diario, marcadas abajo.
   *[other] { $still_listed } siguen en tu diario, marcadas abajo.
    }

journal.window.import.removal.receipt.others.gone =
    { $quantity ->
    [one] la otra ya no aparece aquí.
    [many] las otras ya no aparecen aquí.
   *[other] las otras ya no aparecen aquí.
    }

journal.window.import.removal.receipt.part.erased.note =
    { $quantity ->
    [one] la otra no se borró. búscala abajo o en la papelera.
    [many] las otras no se borraron. búscalas abajo o en la papelera.
   *[other] las otras no se borraron. búscalas abajo o en la papelera.
    }

journal.window.import.removal.receipt.part.recovered.note =
    { $quantity ->
    [one] la otra sigue en la papelera. puedes volver a intentarlo desde ahí.
    [many] las otras siguen en la papelera. puedes volver a intentarlo desde ahí.
   *[other] las otras siguen en la papelera. puedes volver a intentarlo desde ahí.
    }

journal.window.import.removal.receipt.partial.erase =
    { $quantity ->
    [one] { $completed_quantity ->
        [one] se borró { $completed_count } de { $import_count }
        [many] se borraron { $completed_count } de { $import_count }
       *[other] se borraron { $completed_count } de { $import_count }
    }
    [many] { $completed_quantity ->
        [one] se borró { $completed_count } de { $import_count }
        [many] se borraron { $completed_count } de { $import_count }
       *[other] se borraron { $completed_count } de { $import_count }
    }
   *[other] { $completed_quantity ->
        [one] se borró { $completed_count } de { $import_count }
        [many] se borraron { $completed_count } de { $import_count }
       *[other] se borraron { $completed_count } de { $import_count }
    }
    }

journal.window.import.removal.receipt.partial.move =
    { $quantity ->
    [one] { $completed_quantity ->
        [one] se movió { $completed_count } de { $import_count } a la papelera
        [many] se movieron { $completed_count } de { $import_count } a la papelera
       *[other] se movieron { $completed_count } de { $import_count } a la papelera
    }
    [many] { $completed_quantity ->
        [one] se movió { $completed_count } de { $import_count } a la papelera
        [many] se movieron { $completed_count } de { $import_count } a la papelera
       *[other] se movieron { $completed_count } de { $import_count } a la papelera
    }
   *[other] { $completed_quantity ->
        [one] se movió { $completed_count } de { $import_count } a la papelera
        [many] se movieron { $completed_count } de { $import_count } a la papelera
       *[other] se movieron { $completed_count } de { $import_count } a la papelera
    }
    }

journal.window.import.removal.receipt.partial.recover =
    { $quantity ->
    [one] { $completed_quantity ->
        [one] se recuperó { $completed_count } de { $import_count }
        [many] se recuperaron { $completed_count } de { $import_count }
       *[other] se recuperaron { $completed_count } de { $import_count }
    }
    [many] { $completed_quantity ->
        [one] se recuperó { $completed_count } de { $import_count }
        [many] se recuperaron { $completed_count } de { $import_count }
       *[other] se recuperaron { $completed_count } de { $import_count }
    }
   *[other] { $completed_quantity ->
        [one] se recuperó { $completed_count } de { $import_count }
        [many] se recuperaron { $completed_count } de { $import_count }
       *[other] se recuperaron { $completed_count } de { $import_count }
    }
    }

journal.window.import.removal.receipt.recovered.import = importación recuperada

journal.window.import.removal.receipt.recovered.imports =
    { $quantity ->
    [one] { $import_count } importación recuperada
    [many] { $import_count } importaciones recuperadas
   *[other] { $import_count } importaciones recuperadas
    }

journal.window.import.removal.receipt.still =
    { $quantity ->
    [one] { $still_listed } no se pudo mover y sigue en tu diario, marcada abajo.
    [many] { $still_listed } no se pudieron mover y siguen en tu diario, marcadas abajo.
   *[other] { $still_listed } no se pudieron mover y siguen en tu diario, marcadas abajo.
    }

journal.window.import.removal.shared.erase =
    { $quantity ->
    [one] en 1 día, parte de lo que trajo se queda y no se borra, porque una importación anterior de tu diario lo trajo primero. va con esa importación.
    [many] en { $shared_days } días, parte de lo que trajo se queda y no se borra, porque una importación anterior de tu diario lo trajo primero. va con esa importación.
   *[other] en { $shared_days } días, parte de lo que trajo se queda y no se borra, porque una importación anterior de tu diario lo trajo primero. va con esa importación.
    }

journal.window.import.removal.shared.move =
    { $quantity ->
    [one] en 1 día, parte de lo que trajo se queda, porque una importación anterior de tu diario lo trajo primero. va con esa importación.
    [many] en { $shared_days } días, parte de lo que trajo se queda, porque una importación anterior de tu diario lo trajo primero. va con esa importación.
   *[other] en { $shared_days } días, parte de lo que trajo se queda, porque una importación anterior de tu diario lo trajo primero. va con esa importación.
    }

journal.window.import.removal.waiting.many.imports =
    { $quantity ->
    [one] el segmento de ellas que espera por su cuenta en la papelera se borra con ellas.
    [many] los { $waiting_segments } segmentos de ellas que esperan por su cuenta en la papelera se borran con ellas.
   *[other] los { $waiting_segments } segmentos de ellas que esperan por su cuenta en la papelera se borran con ellas.
    }

journal.window.import.removal.waiting.one.import =
    { $quantity ->
    [one] el segmento de ella que espera por su cuenta en la papelera se borra con ella.
    [many] los { $waiting_segments } segmentos de ella que esperan por su cuenta en la papelera se borran con ella.
   *[other] los { $waiting_segments } segmentos de ella que esperan por su cuenta en la papelera se borran con ella.
    }

journal.window.import.response.apple.insufficient.space = esta importación necesita unos{ $slot1 }{ $need }{ $slot2 }libres, y este disco tiene{ $slot3 }{ $free }{ $slot4 }libres, como lo informa el sistema. libera unos{ $slot5 }{ $difference } y luego iníciala.

journal.window.import.response.error.a.required.field.is.missing = falta un campo obligatorio.

journal.window.import.response.error.could.not.read.progress = no se pudo leer el avance de la importación

journal.window.import.response.error.one.of.those.values.couldn.t = no se pudo usar uno de esos valores.

journal.window.import.response.error.something.changed.so.choose.the.source = algo cambió, así que elige la fuente otra vez.

journal.window.import.response.error.that.action.isn.t.available.in = esa acción no está disponible en el estado actual.

journal.window.import.response.error.that.archive.couldn.t.be.previewed = no se pudo hacer la vista previa de ese paquete.

journal.window.import.response.error.that.delete.couldn.t.be.read = no se pudo leer esa eliminación, así que no se eliminó nada.

journal.window.import.response.error.that.file.is.too.large.to = ese archivo es demasiado grande para traerlo.

journal.window.import.response.error.that.file.isn.t.available = ese archivo no está disponible.

journal.window.import.response.error.that.import.couldn.t.be.moved = no se pudo mover esa importación, así que no se eliminó nada.

journal.window.import.response.error.that.import.couldn.t.be.started = no se pudo iniciar esa importación porque ya existe.

journal.window.import.response.error.that.import.isn.t.in.your = esa importación no está en tu diario.

journal.window.import.response.error.that.import.metadata.couldn.t.be = no se pudieron actualizar los metadatos de esa importación.

journal.window.import.response.error.that.import.metadata.couldn.t.be.variant2 = no se pudieron leer los metadatos de esa importación.

journal.window.import.response.error.that.imported.content.couldn.t.be = no se pudo leer ese contenido importado.

journal.window.import.response.error.the.import.could.not.be.prepared = no se pudo preparar la importación.

journal.window.import.response.error.there.are.no.files.to.bring = no hay archivos para traer.

journal.window.import.response.error.there.is.nothing.new.in.this = no hay nada nuevo para importar en esta fuente.

journal.window.import.response.error.there.s.nothing.to.browse.for = no hay nada que explorar en esta importación.

journal.window.import.response.error.this.export.is.over.gb.in = esta exportación tiene más de 2 { $slot1 }. en { $slot2 }, expórtala de nuevo en partes más pequeñas.

journal.window.import.response.error.this.import.can.t.be.deleted = esta importación no se puede eliminar aquí.

journal.window.import.response.error.this.import.has.already.started = esta importación ya se inició.

journal.window.import.response.error.this.import.or.its.source.wasn = no se encontró esta importación o su fuente, así que elige la fuente otra vez.

journal.window.import.response.error.this.import.s.source.or.date = la fuente o la fecha de esta importación no es válida.

journal.window.import.response.error.this.journal.couldn.t.be.exported = no se pudo exportar este diario.

journal.window.import.response.error.this.looks.like.a.strava.download = esto parece una descarga de { $slot1 }, así que tu diario no la conservó. usa la tarjeta de { $slot2 } para que solo se lean tus entrenamientos.

journal.window.import.response.error.your.journal.can.t.open.heic = tu diario no puede abrir fotos { $slot1 } o { $slot2 } directamente. agrégala con la app de solstone en tu teléfono o computadora, o conviértela primero a { $slot3 }.

journal.window.import.response.error.your.journal.couldn.t.finish.that = tu diario no pudo terminar esa eliminación. recarga para revisar.

journal.window.import.response.error.your.journal.couldn.t.move.that = tu diario no pudo mover eso a la papelera, así que no se eliminó nada.

journal.window.import.response.error.your.journal.s.background.service.isn = el servicio en segundo plano de tu diario no está en ejecución. inícialo y luego vuelve a intentarlo.

journal.window.import.response.evernote.count.attachment =
    { $quantity ->
    [one] 1 adjunto
    [many] { $count } adjuntos
   *[other] { $count } adjuntos
    }

journal.window.import.response.evernote.count.note =
    { $quantity ->
    [one] 1 nota
    [many] { $count } notas
   *[other] { $count } notas
    }

journal.window.import.response.evernote.empty = esta exportación no tiene notas.

journal.window.import.response.evernote.ended.empty = esta exportación terminó antes de tiempo. entraron 0 notas.

journal.window.import.response.evernote.finish.already.in.journal = estas notas ya están en tu diario.

journal.window.import.response.evernote.finish.attachments =
    { $quantity ->
    [one] 1 adjunto entró con ellas.
    [many] { $count } adjuntos entraron con ellas.
   *[other] { $count } adjuntos entraron con ellas.
    }

journal.window.import.response.evernote.finish.attachments.single.note =
    { $quantity ->
    [one] 1 adjunto entró con ella.
    [many] { $count } adjuntos entraron con ella.
   *[other] { $count } adjuntos entraron con ella.
    }

journal.window.import.response.evernote.finish.brought.in =
    { $quantity ->
    [one] trajo 1 nota.
    [many] trajo { $count } notas.
   *[other] trajo { $count } notas.
    }

journal.window.import.response.evernote.finish.brought.in.both = { $new_part } { $changed_part }

journal.window.import.response.evernote.finish.brought.in.changed =
    { $quantity ->
    [one] trajo cambios en 1 nota que ya estaba aquí.
    [many] trajo cambios en { $count } notas que ya estaban aquí.
   *[other] trajo cambios en { $count } notas que ya estaban aquí.
    }

journal.window.import.response.evernote.finish.dated.after.today =
    { $quantity ->
    [one] 1 nota está fechada después de hoy y quedó fuera.
    [many] { $count } notas están fechadas después de hoy y quedaron fuera.
   *[other] { $count } notas están fechadas después de hoy y quedaron fuera.
    }

journal.window.import.response.evernote.finish.ended.early =
    { $quantity ->
    [one] esta exportación terminó antes de tiempo. entró 1 nota.
    [many] esta exportación terminó antes de tiempo. entraron { $count } notas.
   *[other] esta exportación terminó antes de tiempo. entraron { $count } notas.
    }

journal.window.import.response.evernote.finish.no.date =
    { $quantity ->
    [one] 1 nota no tiene fecha y quedó fuera.
    [many] { $count } notas no tienen fecha y quedaron fuera.
   *[other] { $count } notas no tienen fecha y quedaron fuera.
    }

journal.window.import.response.evernote.notebook = las notas que comparten este nombre siguen juntas.

journal.window.import.response.evernote.preview.dated = { $notes }, de { $first } a { $last }. { $attachments }.

journal.window.import.response.evernote.preview.encrypted =
    { $quantity ->
    [one] 1 nota tiene una parte cifrada que quedará fuera.
    [many] { $count } notas tienen una parte cifrada que quedará fuera.
   *[other] { $count } notas tienen una parte cifrada que quedará fuera.
    }

journal.window.import.response.evernote.preview.undated = { $notes }. { $attachments }.

journal.window.import.response.evernote.type = este archivo no es una exportación de { $slot1 }.

journal.window.import.response.originals.read = no se pudo cargar esta opción.

journal.window.import.response.originals.unknown.source = fuente de importación desconocida

journal.window.import.response.originals.write = no se pudo guardar esta opción. vuelve a intentarlo.

journal.window.import.response.preview.apple.go.ahead = guardar una exportación de { $slot1 } necesita tu visto bueno, así que no se trajo nada de ella.{ $slot2 }{ $where }: { $slot3 }{ $slot4 }

journal.window.import.response.preview.apple.other.records = registros de { $slot1 }

journal.window.import.response.preview.archive.other.files = otros archivos del paquete

journal.window.import.response.preview.embedded.body = las exportaciones de { $slot1 } y los datos de { $slot2 } los trae un diario en una computadora, así que no se trajo nada.

journal.window.import.response.preview.force.day = aviso: este archivo es del{ $slot1 }{ $fileday }, y una importación previa usó{ $slot2 }{ $previous }. esta importación { $slot3 } usará{ $slot4 }{ $planned }. las grabaciones existentes no se mueven.{ $slot5 }

journal.window.import.response.removal.set.failed = tu diario no pudo moverla a la papelera.

journal.window.import.response.removal.set.held = { $reason }.

journal.window.import.response.removal.set.not.allowed = esta importación no se puede eliminar aquí.

journal.window.import.response.removal.set.unreadable = no se pudo leer una parte de esta importación, así que no se eliminó nada de ella

journal.window.import.section.heading.confirm.import.details = confirmar los detalles de la importación

journal.window.import.section.heading.import.history = historial de importación

journal.window.import.section.heading.quick.import = importación rápida

journal.window.import.section.heading.sync.sources = fuentes de sincronización

journal.window.import.selection.additional =
    { $quantity ->
    [one] y { $additional_count } más
    [many] y { $additional_count } más
   *[other] y { $additional_count } más
    }

journal.window.import.selection.count =
    { $quantity ->
    [one] { $selected_count } seleccionada
    [many] { $selected_count } seleccionadas
   *[other] { $selected_count } seleccionadas
    }

journal.window.import.selection.file.name = selecciona { $file_name }

journal.window.import.source.apple.health.description = importa tus entrenamientos, tu sueño y tus lecturas desde una exportación de { $slot1 }

journal.window.import.source.apple.health.name = { $slot1 }

journal.window.import.source.apple.health.upload.prompt = sube export.zip desde la app Health de tu iphone

journal.window.import.source.audio.archive.description = trae grabaciones { $slot1 } con sus tiempos originales de grabación

journal.window.import.source.audio.archive.name = paquete de audio

journal.window.import.source.audio.archive.upload.prompt = pega la ruta completa de tu carpeta de grabaciones

journal.window.import.source.chatgpt.description = importa tu historial de conversaciones desde { $slot1 }

journal.window.import.source.chatgpt.name = { $slot1 }

journal.window.import.source.chatgpt.upload.prompt = sube el archivo { $slot2 } de tu exportación de { $slot1 }

journal.window.import.source.claude.description = importa tu historial de conversaciones desde { $slot1 }

journal.window.import.source.claude.name = { $slot1 }

journal.window.import.source.claude.upload.prompt = sube el archivo { $slot2 } de tu exportación de { $slot1 }

journal.window.import.source.document.description = importa un documento { $slot1 }

journal.window.import.source.document.name = documento

journal.window.import.source.document.upload.prompt = sube un archivo { $slot1 }

journal.window.import.source.evernote.description = trae notas de una exportación de { $slot1 }

journal.window.import.source.evernote.name = { $slot1 }

journal.window.import.source.evernote.upload.prompt = sube una exportación de { $slot1 }

journal.window.import.source.gemini.description = importa tu historial de actividad desde Google { $slot1 }

journal.window.import.source.gemini.name = { $slot1 }

journal.window.import.source.gemini.upload.prompt = sube tu archivo { $slot1 } { $slot2 }

journal.window.import.source.ics.description = importa eventos de { $slot1 }, { $slot2 } o { $slot3 }

journal.window.import.source.ics.name = calendario

journal.window.import.source.ics.upload.prompt = sube tu archivo { $slot1 } o tu exportación { $slot2 }

journal.window.import.source.image.description = agrega una foto o una captura de pantalla y deja que un modelo describa lo que hay en ella

journal.window.import.source.image.name = imagen

journal.window.import.source.image.upload.prompt = sube una imagen ({ $slot1 }, { $slot2 }, { $slot3 }, { $slot4 })

journal.window.import.source.journal.archive.description = importa una exportación completa de un diario desde otro diario

journal.window.import.source.journal.archive.name = diario

journal.window.import.source.journal.archive.upload.prompt = sube el archivo { $slot1 } de la exportación de tu diario

journal.window.import.source.quick.description = pega texto o suelta cualquier archivo para una importación rápida

journal.window.import.source.quick.name = importación rápida

journal.window.import.source.quick.upload.prompt = pega texto o arrastra y suelta un archivo

journal.window.import.source.recording.description = importa audio de reuniones o conversaciones

journal.window.import.source.recording.name = audio de reunión

journal.window.import.source.recording.upload.prompt = sube un archivo de audio, video o imagen

journal.window.import.source.strava.description = importa tus entrenamientos desde una descarga de { $slot1 }

journal.window.import.source.strava.name = { $slot1 }

journal.window.import.source.strava.upload.prompt = sube tu descarga { $slot2 } de { $slot1 }, o solo { $slot3 } desde dentro de ella

journal.window.import.stage.names.audio.segmenting.splitting.audio = dividiendo el audio…

journal.window.import.stage.names.cleanup.finishing.up = terminando…

journal.window.import.stage.names.detection.detecting.content.type = detectando el tipo de contenido…

journal.window.import.stage.names.entities.merging.entities = fusionando entidades…

journal.window.import.stage.names.execution.reading.the.import = leyendo la importación…

journal.window.import.stage.names.facets.merging.facets = fusionando facetas…

journal.window.import.stage.names.importing.reading.the.import = leyendo la importación…

journal.window.import.stage.names.imports.copying.prior.imports = copiando importaciones anteriores…

journal.window.import.stage.names.indexing.building.search.index = construyendo el índice de búsqueda…

journal.window.import.stage.names.initialization.preparing = preparando…

journal.window.import.stage.names.processing.processing.entries = procesando entradas…

journal.window.import.stage.names.segmenting.preparing.segments = preparando segmentos…

journal.window.import.stage.names.segments.merging.segments = fusionando segmentos…

journal.window.import.stage.names.transcribing.transcribing.audio = transcribiendo el audio…

journal.window.import.stage.names.transcription.transcribing.audio = transcribiendo el audio…

journal.window.import.stage.names.writing.writing.to.journal = escribiendo en el diario…

journal.window.import.status.completed = completado

journal.window.import.status.conflict.already.imported = ya importaste este archivo. si eliminaste una parte, recupérala desde la papelera, o elimina toda la importación y vuelve a importar el archivo.

journal.window.import.status.conflict.already.started = esta importación ya se inició. revisa su avance en tus importaciones.

journal.window.import.status.conflict.changed = esta importación cambió. actualiza la página y vuelve a intentarlo.

journal.window.import.status.error.failed = no se pudo importar

journal.window.import.status.error.never.completed = La importación nunca se completó

journal.window.import.status.error.unavailable = estado de la importación no disponible

journal.window.import.status.error.unconfirmed = no se pudo confirmar que esta importación terminó.

journal.window.import.status.error.with.stage = { $error } ({ $stage })

journal.window.import.status.failed = falló

journal.window.import.status.pending = pendiente

journal.window.import.status.running = en ejecución

journal.window.import.status.stage.execution = Ejecución

journal.window.import.status.stage.finalization = Finalización

journal.window.import.status.stage.interrupted = Interrumpido

journal.window.import.status.stage.publication = Publicación

journal.window.import.status.stage.storage = Almacenamiento

journal.window.import.status.stage.timeout = Tiempo de espera

journal.window.import.status.status.unavailable = estado no disponible

journal.window.import.status.stopped = detenido

journal.window.import.status.text.any.supported.file = cualquier archivo compatible

journal.window.import.status.text.failed = falló

journal.window.import.status.text.paste.the.full.path.to.your = pega la ruta completa de tu carpeta

journal.window.import.status.text.please.provide.a.file.or.paste = Por favor, proporciona un archivo o pega texto

journal.window.import.status.text.select.a.file.to.import = selecciona un archivo para importar

journal.window.import.status.text.the.date.is.required.before.starting = la fecha es obligatoria antes de iniciar la importación.

journal.window.import.status.text.unknown.error = error desconocido

journal.window.import.status.text.validation.failed = Falló la validación

journal.window.import.status.unconfirmed = sin confirmar

journal.window.import.strings.activities.activities = actividades

journal.window.import.strings.artifact.paths.artifact.paths = rutas de artefactos

journal.window.import.strings.body.didnt.start.didn.t.start = no se inició

journal.window.import.strings.body.failed.not.brought.in = no se trajo

journal.window.import.strings.body.partly.saved.partly.saved = guardado en parte

journal.window.import.strings.body.stopped.stopped = detenido

journal.window.import.strings.check.status.check.status = revisar el estado

journal.window.import.strings.checking.status.checking.status = revisando el estado…

journal.window.import.strings.collision.title.owner.identity.differs.between.journals = la identidad del propietario no coincide entre los diarios

journal.window.import.strings.completed.at.completed.at = completado el

journal.window.import.strings.completed.completed = completado

journal.window.import.strings.created.files.created.files = archivos creados

journal.window.import.strings.date.range.date.range = intervalo de fechas

journal.window.import.strings.date.source.date.source = fuente de la fecha

journal.window.import.strings.date.source.fallback.import.time.no.date.in.photo = hora de la importación (sin fecha en la foto)

journal.window.import.strings.date.source.owner.chosen.by.you = elegida por ti

journal.window.import.strings.date.source.photo.from.photo = de la foto

journal.window.import.strings.decisions.decisions = decisiones

journal.window.import.strings.delete.import.delete.this.import = eliminar esta importación

journal.window.import.strings.detected.time.detected.time = hora detectada

journal.window.import.strings.drawer.label.bookkeeping = contabilidad

journal.window.import.strings.drawer.unavailable.drawer.renderer.unavailable = el renderizador del cajón no está disponible

journal.window.import.strings.entities.entities = entidades

journal.window.import.strings.entries.entries = entradas

journal.window.import.strings.error.error = error

journal.window.import.strings.facets.facets = facetas

journal.window.import.strings.failed.at.failed.at = falló el

journal.window.import.strings.failed.failed = falló

journal.window.import.strings.failed.line.failed.while.processing = falló durante el procesamiento

journal.window.import.strings.failed.stage.failed.stage = etapa fallida

journal.window.import.strings.file.file = archivo

journal.window.import.strings.file.size.file.size = tamaño del archivo

journal.window.import.strings.files.created.files.created = archivos creados

journal.window.import.strings.files.files = archivos

journal.window.import.strings.has.gaps.contains.gaps = contiene huecos

journal.window.import.strings.identity.differs.identity.differs = la identidad es distinta

journal.window.import.strings.image.description.gap.detail.image.saved.without.visual.description = imagen guardada sin descripción visual

journal.window.import.strings.import.time.import.time = hora de la importación

journal.window.import.strings.imports.imports = importaciones

journal.window.import.strings.leads.title.where.this.landed = dónde quedó esto

journal.window.import.strings.merge.highlights.merge.highlights = destacados de la fusión

journal.window.import.strings.merge.summary.merge.summary = resumen de la fusión

journal.window.import.strings.mime.type.mime.type = tipo mime

journal.window.import.strings.not.started.not.started = sin iniciar

journal.window.import.strings.nothing.to.import.nothing.to.import = nada que importar

journal.window.import.strings.original.file.original.file = archivo original

journal.window.import.strings.owner.identity.differs.owner.identity.differs = la identidad del propietario no coincide

journal.window.import.strings.pending.pending = pendiente

journal.window.import.strings.photo.fallback.notice.no.date.in.photo.placed.at = sin fecha en la foto · colocada a la hora de la importación

journal.window.import.strings.preparing.preview.preparing.preview = preparando la vista previa…

journal.window.import.strings.preview.failed.couldn.t.prepare.a.preview.try = no se pudo preparar una vista previa. vuelve a intentarlo.

journal.window.import.strings.preview.import.preview.import = previsualizar la importación

journal.window.import.strings.processing.facts.processing.facts = hechos del procesamiento

journal.window.import.strings.processing.processing = procesando…

journal.window.import.strings.profile.link.review.profile.settings = revisar la configuración del perfil

journal.window.import.strings.raw.payload.raw.payload = carga útil en bruto

journal.window.import.strings.running.running = en ejecución

journal.window.import.strings.saved.saved = guardado

journal.window.import.strings.segments.segments = segmentos

journal.window.import.strings.setting.setting = contexto

journal.window.import.strings.source.source = fuente

journal.window.import.strings.staged.entities.staged.entities = entidades preparadas

journal.window.import.strings.staging.staging = preparación

journal.window.import.strings.start.import.start.import = iniciar la importación

journal.window.import.strings.start.unconfirmed.couldn.t.confirm.whether.the.import = no se pudo confirmar si la importación se inició. revisa el estado antes de hacer cualquier otra cosa.

journal.window.import.strings.start.unknown.detail.this.file.is.saved.but.its = este archivo está guardado, pero su historial no confirma si la importación se inició.

journal.window.import.strings.start.unknown.start.status.unknown = estado de inicio desconocido

journal.window.import.strings.starting.starting = iniciando…

journal.window.import.strings.status.failed.couldn.t.load.the.import.s = no se pudo cargar el estado de la importación. revisa de nuevo.

journal.window.import.strings.status.status = estado

journal.window.import.strings.status.unavailable.import.status.unavailable = estado de la importación no disponible

journal.window.import.strings.summary.errors.summary.errors = errores del resumen

journal.window.import.strings.target.day.target.day = día de destino

journal.window.import.strings.unavailable.description.unavailable.description = descripción no disponible

journal.window.import.strings.unavailable.pages.unavailable.pages = páginas no disponibles

journal.window.import.strings.unconfirmed.explanation.this.import.couldn.t.be.confirmed = no se pudo confirmar que esta importación terminó.

journal.window.import.strings.unconfirmed.import.unconfirmed = importación sin confirmar

journal.window.import.strings.untimed.ask.ask.us.to.add.it = pídenos que lo agreguemos

journal.window.import.strings.untimed.notice.no.turn.times.found.in.this = no hay tiempos de turno en este archivo, así que quedó como un segmento a la hora de inicio de la importación, cada línea tal cual. si tus transcripciones marcan tiempos de un modo que el diario todavía no lee,

journal.window.import.strings.upload.facts.upload.facts = hechos de la subida

journal.window.import.strings.uploaded.at.uploaded.at = subido el

journal.window.import.strings.view.day.view.that.day = ver ese día

journal.window.import.unit.record = registro

journal.window.import.unit.records = registros

journal.window.import.upload.accepted.formats = acepta { $formats }

journal.window.import.upload.duplicate.running = esta importación ya está en curso.

journal.window.import.upload.duplicate.running.named = esta importación ya está en curso como { $import_id }.

journal.window.import.upload.duplicate.saved = este archivo ya estaba preparado.

journal.window.import.upload.duplicate.saved.named = este archivo ya estaba preparado como { $import_id }.

journal.window.import.upload.saved.named = { $name }: se guardó

journal.window.import.upload.saved.setup.unconfirmed = { $name }: se guardó, pero no se pudo confirmar la configuración de la importación. revisa tus importaciones antes de volver a elegir.

journal.window.import.visible.text.a.short.note.about.what.this = una nota breve sobre qué es esto, para encontrarlo más fácil después

journal.window.import.visible.text.checking.import = revisando la importación…

journal.window.import.visible.text.completed = completado

journal.window.import.visible.text.configure.and.start = configurar e iniciar

journal.window.import.visible.text.contains.gaps = contiene huecos

journal.window.import.visible.text.content = contenido

journal.window.import.visible.text.couldn.t.check.status.reload.to = no se pudo revisar el estado. recarga para volver a intentarlo.

journal.window.import.visible.text.couldn.t.load.import.details = no se pudieron cargar los detalles de la importación.

journal.window.import.visible.text.couldn.t.load.more.imports = no se pudieron cargar más importaciones.

journal.window.import.visible.text.couldn.t.load.this.choice.reopen = no se pudo cargar esta opción. reabre la importación para volver a intentarlo.

journal.window.import.visible.text.couldn.t.save.this.choice.try = no se pudo guardar esta opción. vuelve a intentarlo.

journal.window.import.visible.text.date.range = intervalo de fechas:

journal.window.import.visible.text.drop.a.file.here.or.choose = suelta un archivo aquí o elige uno

journal.window.import.visible.text.duration = duración:

journal.window.import.visible.text.entities.seeded = entidades iniciales:

journal.window.import.visible.text.entries.written = entradas escritas:

journal.window.import.visible.text.error = error:

journal.window.import.visible.text.error.loading.content = error al cargar el contenido

journal.window.import.visible.text.error.loading.content.variant2 = Error al cargar el contenido

journal.window.import.visible.text.folder.saved.but.import.setup.could = carpeta guardada, pero no se pudo confirmar la configuración de importación. revisa tus importaciones antes de elegirla otra vez.

journal.window.import.visible.text.import.id = id de la importación:

journal.window.import.visible.text.leave.this.empty.to.use.recording = deja esto vacío para usar los nombres de archivo de las grabaciones. el manifiesto y sus grabaciones deben estar juntos en la carpeta elegida. ese puede especificar sus propios tiempos.

journal.window.import.visible.text.loading = cargando…

journal.window.import.visible.text.loading.content = cargando el contenido…

journal.window.import.visible.text.loading.imports = cargando importaciones…

journal.window.import.visible.text.no.content.available = No hay contenido disponible

journal.window.import.visible.text.no.creation.date.was.found.in = no se encontró una fecha de creación en este archivo. elige su fecha, o conserva la fecha de subida.

journal.window.import.visible.text.no.items.found = no se encontraron elementos

journal.window.import.visible.text.optional = (opcional)

journal.window.import.visible.text.original.saved.description.unavailable = original guardado (descripción no disponible)

journal.window.import.visible.text.overview = resumen

journal.window.import.visible.text.overview.unavailable = resumen no disponible

journal.window.import.visible.text.paste.text.or.drop.a.file = pega texto o suelta un archivo. vas a confirmar la fecha antes de que entre.

journal.window.import.visible.text.preparing.preview = preparando la vista previa…

journal.window.import.visible.text.preview.import = previsualizar la importación

journal.window.import.visible.text.preview.then.merge.your.archive = previsualiza y luego fusiona tu paquete

journal.window.import.visible.text.preview.your.recordings = previsualiza tus grabaciones

journal.window.import.visible.text.progress = progreso

journal.window.import.visible.text.ready.to.sync.recordings.into.your = ya se pueden sincronizar las grabaciones en tu diario.

journal.window.import.visible.text.saving.folder.this.can.take.a = guardando la carpeta… esto puede tardar un rato con una carpeta grande.

journal.window.import.visible.text.select.this.tab.to.browse.imported = selecciona esta pestaña para explorar el contenido importado

journal.window.import.visible.text.stalled = sin avance

journal.window.import.visible.text.stalled.no.updates.in.minutes = sin avance: no hay actualizaciones en 10 minutos.

journal.window.import.visible.text.start.import = iniciar la importación

journal.window.import.visible.text.starting = iniciando

journal.window.import.visible.text.status.unavailable = estado no disponible

journal.window.import.visible.text.still.working = todavía trabajando…

journal.window.import.visible.text.the.zip.includes.your.settings.and = el ZIP incluye tu configuración y tus credenciales. quedan fuera registros de procesamiento, índices y cachés reconstruibles, bloqueos y archivos temporales. fusionar conserva la configuración y las conexiones del diario receptor.

journal.window.import.visible.text.this.import.could.not.be.confirmed = no se pudo confirmar esta importación. revisa tus importaciones antes de elegirla otra vez.

journal.window.import.visible.text.this.import.couldn.t.be.confirmed = no se pudo confirmar que esta importación terminó.

journal.window.import.visible.text.this.pdf.uses.its.own.dates = este { $slot1 } usa sus propias fechas. puedes elegir una fecha para este archivo.

journal.window.import.visible.text.turn.this.on.if.you.want = activa esto si quieres una copia local de los originales en tu diario.

journal.window.import.visible.text.unconfirmed = sin confirmar

journal.window.import.visible.text.unknown.import.source = fuente de importación desconocida

journal.window.import.visible.text.upload.your.file = sube tu archivo

journal.window.import.visible.text.uploading.your.file.keep.this.page = subiendo tu archivo… mantén esta página abierta hasta que termine.

journal.window.import.visible.text.validate = validar

journal.window.import.visible.text.validating = validando

journal.window.import.window.explanation.could.not.preview.this.import = no se pudo previsualizar esta importación

journal.window.import.window.explanation.failed.to.save.import = No se pudo guardar la importación

journal.window.import.window.explanation.failed.to.save.import.settings = No se pudo guardar la configuración de la importación

journal.window.import.window.explanation.failed.to.start.import = No se pudo iniciar la importación

journal.window.import.window.explanation.malformed.import.history.response = Respuesta mal formada del historial de importación

journal.window.import.window.explanation.stalled.no.updates.in.minutes = sin avance: no hay actualizaciones en 10 minutos

journal.window.import.window.explanation.thinking.progress.is.unavailable = el progreso del razonamiento no está disponible

journal.window.ingest.notice.app.newer = la app de solstone de ese dispositivo es más nueva que tu diario. actualiza tu diario.

journal.window.ingest.notice.app.older = la app de solstone de ese dispositivo es demasiado antigua para tu diario. actualiza la app de solstone en ese dispositivo.

journal.window.ingest.notice.cleared.named = tu diario vuelve a aceptar subidas de { $device_name }

journal.window.ingest.notice.cleared.unnamed = tu diario vuelve a aceptar subidas de uno de tus dispositivos

journal.window.ingest.notice.raised.named = tu diario está rechazando subidas de { $device_name }

journal.window.ingest.notice.raised.unnamed = tu diario está rechazando subidas de uno de tus dispositivos

journal.window.ingest.notice.tmux.newer = solstone para tmux en ese dispositivo es más nuevo que tu diario. actualiza tu diario.

journal.window.ingest.notice.tmux.older = solstone para tmux en ese dispositivo es demasiado antiguo para tu diario. actualiza solstone para tmux.

journal.window.link.absent = no está en tu diario.

journal.window.link.back = ← atrás

journal.window.link.heading = no se puede abrir esta fuente

journal.window.link.invalid = tu diario no va a seguir este enlace.

journal.window.link.open.journal = abrir tu diario

journal.window.link.read.error = tu diario no pudo revisar esta fuente.

journal.window.link.try.again = reintentar

journal.window.link.unsupported = tu diario no puede mostrar este tipo de fuente.

journal.window.link.wrong.journal.detail = puede ser para otro diario, o el enlace puede haber cambiado.

journal.window.link.wrong.journal.heading = este enlace no se puede abrir aquí.

journal.window.stored.body.summary.activities.no.facet =

    { $quantity ->
        [one] 1 entrenamiento, noche de sueño o sesión de atención plena no aparece en la página de actividades, porque esa parte de tu diario no tiene faceta.
        [many] { $count } de los entrenamientos, las noches de sueño y las sesiones de atención plena no aparecen en la página de actividades, porque esas partes de tu diario no tienen faceta.
       *[other] { $count } de los entrenamientos, las noches de sueño y las sesiones de atención plena no aparecen en la página de actividades, porque esas partes de tu diario no tienen faceta.
    }

journal.window.stored.body.summary.activities.refreshed =

    { $quantity ->
        [one] 1 entrenamiento, noche de sueño o sesión de atención plena cambió sus horas o su tipo en esta importación, y ahora muestra lo nuevo.
        [many] { $count } de los entrenamientos, las noches de sueño y las sesiones de atención plena cambiaron sus horas o su tipo en esta importación, y ahora muestran lo nuevo.
       *[other] { $count } de los entrenamientos, las noches de sueño y las sesiones de atención plena cambiaron sus horas o su tipo en esta importación, y ahora muestran lo nuevo.
    }

journal.window.stored.body.summary.activities.refused = Algunos entrenamientos, noches de sueño o sesiones de atención plena no se pudieron mostrar bajo { $facets }. La próxima importación intenta otra vez.

journal.window.stored.body.summary.activities.shown =

    { $quantity ->
        [one] 1 entrenamiento, noche de sueño o sesión de atención plena ahora se muestra en su día.
        [many] { $count } de los entrenamientos, las noches de sueño y las sesiones de atención plena ahora se muestran en sus días.
       *[other] { $count } de los entrenamientos, las noches de sueño y las sesiones de atención plena ahora se muestran en sus días.
    }

journal.window.stored.body.summary.counts.base = { $written } escritas, { $unchanged } sin cambios, { $unreadable } ilegibles y sin tocar.

journal.window.stored.body.summary.counts.stayed = { $written } escritas, { $unchanged } sin cambios, { $stayed_deleted } siguieron eliminadas, { $unreadable } ilegibles y sin tocar.

journal.window.stored.body.summary.deferred =

    { $quantity ->
        [one] { $source } movió 1 entrada al otro lado de la medianoche; tu diario conserva la hora que guardó primero.
        [many] { $source } movió { $count } entradas al otro lado de la medianoche; tu diario conserva las horas que guardó primero.
       *[other] { $source } movió { $count } entradas al otro lado de la medianoche; tu diario conserva las horas que guardó primero.
    }

journal.window.stored.body.summary.earlier.waiting =

    { $quantity ->
        [one] 1 entrenamiento anterior de Strava todavía no se pudo actualizar, así que puede aparecer dos veces junto a lo que agregó esta importación hasta que se actualice.
        [many] { $count } entrenamientos anteriores de Strava todavía no se pudieron actualizar, así que pueden aparecer dos veces junto a lo que agregó esta importación hasta que se actualicen.
       *[other] { $count } entrenamientos anteriores de Strava todavía no se pudieron actualizar, así que pueden aparecer dos veces junto a lo que agregó esta importación hasta que se actualicen.
    }

journal.window.stored.body.summary.entries.stayed.deleted =

    { $quantity ->
        [one] 1 entrada no se agregó, porque eliminaste la parte del diario a la que pertenece.
        [many] { $count } entradas no se agregaron, porque eliminaste la parte del diario a la que pertenecen.
       *[other] { $count } entradas no se agregaron, porque eliminaste la parte del diario a la que pertenecen.
    }

journal.window.stored.body.summary.facet.chosen = las entradas nuevas se archivan bajo la faceta { $facet }.

journal.window.stored.body.summary.facet.none = no se eligió una faceta para esta importación. las entradas sin faceta son legibles para un agente que puede leer todo tu diario, y no para uno limitado a facetas elegidas.

journal.window.stored.body.summary.facets.another = otra faceta

journal.window.stored.body.summary.facets.earlier.only = un archivado anterior

journal.window.stored.body.summary.facets.many = tus facetas { $rest } y { $last }

journal.window.stored.body.summary.facets.one = tu faceta { $facet }

journal.window.stored.body.summary.facets.with.earlier = { $named } y un archivado anterior

journal.window.stored.body.summary.heading = # importación de { $source }

journal.window.stored.body.summary.held.back.reading =

    { $quantity ->
        [one] 1 lectura no se agregó, porque no se pudo leer la parte del diario a la que pertenece.
        [many] { $count } lecturas no se agregaron, porque no se pudo leer la parte del diario a la que pertenecen.
       *[other] { $count } lecturas no se agregaron, porque no se pudo leer la parte del diario a la que pertenecen.
    }

journal.window.stored.body.summary.hole.converging = - { $endpoint }, { $place }: cambió en Oura desde entonces, y el diario no ha terminado de mover esto a su nueva hora; una importación que pide este día otra vez termina ese movimiento

journal.window.stored.body.summary.hole.deferred = - { $endpoint }, { $place }: Oura movió esto al otro lado de la medianoche; tu diario conserva la hora que guardó primero

journal.window.stored.body.summary.hole.held.back = - { $endpoint }, { $place }: todavía sin agregar: puede que ya esté en una parte del diario que no se pudo leer; una importación que pide este día otra vez intenta otra vez

journal.window.stored.body.summary.hole.moved.far = - { $endpoint }, { $place }: Oura movió esto más de un día; tu diario conserva esto donde se guardó primero

journal.window.stored.body.summary.hole.record.refused = - { $endpoint }, { $place }: no se pudo mostrar en su día; una importación que pide este día otra vez intenta otra vez

journal.window.stored.body.summary.hole.unreadable = - { $endpoint }, { $place }: no se pudo leer la parte del diario a la que corresponde; una importación que pide este día otra vez intenta otra vez

journal.window.stored.body.summary.joined.hidden =

    { $quantity ->
        [one] 1 registro se unió a un entrenamiento que habías ocultado, así que también queda oculto.
        [many] { $count } registros se unieron a un entrenamiento que habías ocultado, así que también quedan ocultos.
       *[other] { $count } registros se unieron a un entrenamiento que habías ocultado, así que también quedan ocultos.
    }

journal.window.stored.body.summary.kept.facet =

    { $quantity ->
        [one] 1 parte de tu diario ya lleva una faceta de una importación anterior, así que lo que agregó esta importación también es legible bajo esa faceta.
        [many] { $count } partes de tu diario ya llevan una faceta de una importación anterior, así que lo que agregó esta importación también es legible bajo esa faceta.
       *[other] { $count } partes de tu diario ya llevan una faceta de una importación anterior, así que lo que agregó esta importación también es legible bajo esa faceta.
    }

journal.window.stored.body.summary.kept.unfiled = algunas entradas anteriores se quedaron sin faceta. solo un agente que puede leer todo tu diario puede leerlas.

journal.window.stored.body.summary.listed.moved =

    { $quantity ->
        [one] 1 registro que ya aparece en la página de actividades pasó a la faceta de esta importación, desde { $facets }.
        [many] { $count } registros que ya aparecen en la página de actividades pasaron a la faceta de esta importación, desde { $facets }.
       *[other] { $count } registros que ya aparecen en la página de actividades pasaron a la faceta de esta importación, desde { $facets }.
    }

journal.window.stored.body.summary.listed.over.none.none.shown =

    { $quantity ->
        [one] nada de lo ya listado se movió: se habría movido 1 registro, y no se te mostró ninguno.
        [many] nada de lo ya listado se movió: se habrían movido { $count } registros, y no se te mostró ninguno.
       *[other] nada de lo ya listado se movió: se habrían movido { $count } registros, y no se te mostró ninguno.
    }

journal.window.stored.body.summary.listed.over.none.shown =

    { $quantity ->
        [one] nada de lo ya listado se movió: se habría movido 1 registro, y se te dijo { $shown }.
        [many] nada de lo ya listado se movió: se habrían movido { $count } registros, y se te dijo { $shown }.
       *[other] nada de lo ya listado se movió: se habrían movido { $count } registros, y se te dijo { $shown }.
    }

journal.window.stored.body.summary.listed.over.some.none.shown =

    { $quantity ->
        [one] el resto de lo ya listado se quedó donde estaba: en total se habría movido 1 registro, y no se te mostró ninguno.
        [many] el resto de lo ya listado se quedó donde estaba: en total se habrían movido { $count } registros, y no se te mostró ninguno.
       *[other] el resto de lo ya listado se quedó donde estaba: en total se habrían movido { $count } registros, y no se te mostró ninguno.
    }

journal.window.stored.body.summary.listed.over.some.shown =

    { $quantity ->
        [one] el resto de lo ya listado se quedó donde estaba: en total se habría movido 1 registro, y se te dijo { $shown }.
        [many] el resto de lo ya listado se quedó donde estaba: en total se habrían movido { $count } registros, y se te dijo { $shown }.
       *[other] el resto de lo ya listado se quedó donde estaba: en total se habrían movido { $count } registros, y se te dijo { $shown }.
    }

journal.window.stored.body.summary.metric.daily.activity = actividad diaria

journal.window.stored.body.summary.metric.daily.cardiovascular.age = edad cardiovascular

journal.window.stored.body.summary.metric.daily.readiness = preparación

journal.window.stored.body.summary.metric.daily.resilience = resiliencia

journal.window.stored.body.summary.metric.daily.sleep = puntuaciones de sueño

journal.window.stored.body.summary.metric.daily.spo2 = oxígeno en sangre

journal.window.stored.body.summary.metric.daily.stress = estrés

journal.window.stored.body.summary.metric.heartrate = frecuencia cardíaca

journal.window.stored.body.summary.metric.rest.mode.period = periodos de descanso

journal.window.stored.body.summary.metric.ring.configuration = detalles del anillo

journal.window.stored.body.summary.metric.session = sesiones

journal.window.stored.body.summary.metric.sleep.time = horas de dormir sugeridas

journal.window.stored.body.summary.metric.tags = etiquetas

journal.window.stored.body.summary.metric.vo2.max = VO2 max

journal.window.stored.body.summary.metric.workout = entrenamientos

journal.window.stored.body.summary.moved =

    { $quantity ->
        [one] { $source } ha cambiado desde entonces la hora de 1 entrada; el diario ahora la muestra en la hora nueva.
        [many] { $source } ha cambiado desde entonces las horas de { $count } entradas; el diario ahora las muestra en las horas nuevas.
       *[other] { $source } ha cambiado desde entonces las horas de { $count } entradas; el diario ahora las muestra en las horas nuevas.
    }

journal.window.stored.body.summary.moved.far =

    { $quantity ->
        [one] { $source } movió 1 entrada más de un día; tu diario la conserva donde se guardó primero.
        [many] { $source } movió { $count } entradas más de un día; tu diario las conserva donde se guardaron primero.
       *[other] { $source } movió { $count } entradas más de un día; tu diario las conserva donde se guardaron primero.
    }

journal.window.stored.body.summary.oura.days.heartrate = { $from } a { $to }

journal.window.stored.body.summary.oura.days.other = los días de Oura { $from } a { $to }

journal.window.stored.body.summary.oura.empty = - { $endpoint }: vacío: nada para { $days }

journal.window.stored.body.summary.oura.failed = - { $endpoint }: falló: { $error }

journal.window.stored.body.summary.oura.failed.part.way = - { $endpoint }: falló a medias: se leyó por completo hasta { $through }, luego { $error }

journal.window.stored.body.summary.oura.read.entries =

    { $quantity ->
        [one] - { $endpoint }: leída: 1 entrada para { $days }
        [many] - { $endpoint }: leídas: { $count } entradas para { $days }
       *[other] - { $endpoint }: leídas: { $count } entradas para { $days }
    }

journal.window.stored.body.summary.oura.read.heartrate =

    { $quantity ->
        [one] - { $endpoint }: leída: 1 lectura para { $days }
        [many] - { $endpoint }: leídas: { $count } lecturas para { $days }
       *[other] - { $endpoint }: leídas: { $count } lecturas para { $days }
    }

journal.window.stored.body.summary.oura.refusal.documented = esto necesita el permiso "{ $scope }"; para traer esto, conecta Oura otra vez y permítelo

journal.window.stored.body.summary.oura.refusal.named = Oura dice que esto necesita el permiso { $scopes }; para traer esto, conecta Oura otra vez y permítelo

journal.window.stored.body.summary.oura.refusal.not.named = Oura no dijo qué permiso necesita esto

journal.window.stored.body.summary.oura.refusal.said = Oura dijo: { $detail }

journal.window.stored.body.summary.oura.refused = - { $endpoint }: rechazó: { $why }

journal.window.stored.body.summary.oura.refused.part.way = - { $endpoint }: rechazó a medias: se leyó por completo hasta { $through }, luego { $why }

journal.window.stored.body.summary.oura.through.day = el día de Oura { $day }

journal.window.stored.body.summary.overlapping.nights = { $count } de las noches de Oura en esta importación se superponen con otra. cada una se conserva como Oura la dio, así que si Oura archivó una noche cambiada como una nueva, aparecen las dos.

journal.window.stored.body.summary.placed.zone = Días y horas colocados usando { $zone }.

journal.window.stored.body.summary.refiled =

    { $quantity ->
        [one] 1 parte de tu diario ya guardaba registros, y esos ahora están archivados bajo esta faceta en lugar de la que tenían.
        [many] { $count } partes de tu diario ya guardaban registros, y esos ahora están archivados bajo esta faceta en lugar de la que tenían.
       *[other] { $count } partes de tu diario ya guardaban registros, y esos ahora están archivados bajo esta faceta en lugar de la que tenían.
    }

journal.window.stored.body.summary.replaced = Esta importación fue reemplazada por una posterior antes de que terminara.

journal.window.stored.body.summary.retained.revisions =

    { $quantity ->
        [one] 1 entrada actualizada se quedó bajo { $facets }, donde ya estaba archivada.
        [many] { $count } entradas actualizadas se quedaron bajo { $facets }, donde ya estaban archivadas.
       *[other] { $count } entradas actualizadas se quedaron bajo { $facets }, donde ya estaban archivadas.
    }

journal.window.stored.body.summary.section.couldnt.be.read = ## no se pudo leer

journal.window.stored.body.summary.section.from.oura = ## De Oura

journal.window.stored.body.summary.section.held.back = ## Retenido

journal.window.stored.body.summary.section.held.back.intro = Todavía sin agregar: puede que cada uno ya esté en una parte del diario que no se pudo leer. La próxima importación intenta otra vez.

journal.window.stored.body.summary.section.not.read = ## sin leer

journal.window.stored.body.summary.section.not.saved.yet = ## todavía sin guardar

journal.window.stored.body.summary.section.not.written.in.full = ## No escrito por completo

journal.window.stored.body.summary.section.not.written.in.full.intro = Oura respondió por estos, y el diario no pudo escribir todo.

journal.window.stored.body.summary.section.read = ## leído

journal.window.stored.body.summary.split.rows =

    { $quantity ->
        [one] 1 entrada nueva se guardó por separado de entradas anteriores en el mismo lapso de tiempo, así que cada una conserva su propia faceta.
        [many] { $count } entradas nuevas se guardaron por separado de entradas anteriores en el mismo lapso de tiempo, así que cada una conserva su propia faceta.
       *[other] { $count } entradas nuevas se guardaron por separado de entradas anteriores en el mismo lapso de tiempo, así que cada una conserva su propia faceta.
    }

journal.window.stored.body.summary.stage.end.before.start = terminó antes de empezar

journal.window.stored.body.summary.stage.row.too.large = demasiado grande para conservar

journal.window.stored.body.summary.stage.span.too.long = cubre un tramo de tiempo demasiado largo

journal.window.stored.body.summary.stage.time = sin una hora que el diario pudiera leer

journal.window.stored.body.summary.stage.type = sin un tipo que el diario pudiera leer

journal.window.stored.body.summary.strava.added.and.updated =

    { $quantity ->
        [one] { $added }, 1 entrenamiento cambió desde la última importación y quedó al día.
        [many] { $added }, { $updated } entrenamientos cambiaron desde la última importación y quedaron al día.
       *[other] { $added }, { $updated } entrenamientos cambiaron desde la última importación y quedaron al día.
    }

journal.window.stored.body.summary.strava.added.count =

    { $quantity ->
        [one] 1 entrenamiento agregado
        [many] { $count } entrenamientos agregados
       *[other] { $count } entrenamientos agregados
    }

journal.window.stored.body.summary.strava.added.none = ningún entrenamiento nuevo.

journal.window.stored.body.summary.strava.added.only =

    { $quantity ->
        [one] 1 entrenamiento agregado.
        [many] { $count } entrenamientos agregados.
       *[other] { $count } entrenamientos agregados.
    }

journal.window.stored.body.summary.strava.conversion.moves.open =

    { $quantity ->
        [one] todavía no se pudo terminar de mover 1 registro. tu diario intenta otra vez la próxima vez que inicia, y puedes editarlo después.
        [many] todavía no se pudieron terminar de mover { $count } registros. tu diario intenta otra vez la próxima vez que inicia, y puedes editarlos después.
       *[other] todavía no se pudieron terminar de mover { $count } registros. tu diario intenta otra vez la próxima vez que inicia, y puedes editarlos después.
    }

journal.window.stored.body.summary.strava.conversion.no.workout =

    { $quantity ->
        [one] 1 registro anterior se dejó sin cambios porque tu diario no pudo confirmar a qué entrenamiento pertenecía.
        [many] { $count } registros anteriores se dejaron sin cambios porque tu diario no pudo confirmar a qué entrenamientos pertenecían.
       *[other] { $count } registros anteriores se dejaron sin cambios porque tu diario no pudo confirmar a qué entrenamientos pertenecían.
    }

journal.window.stored.body.summary.strava.conversion.waiting = algunos entrenamientos anteriores todavía no se pudieron actualizar. tu diario intenta otra vez la próxima vez que inicia.

journal.window.stored.body.summary.strava.facet.chosen = elegiste la faceta { $facet } para esta importación.

journal.window.stored.body.summary.strava.facet.chosen.none = no elegiste una faceta para esta importación: un entrenamiento sin faceta no aparece en la página de actividades, no salen personas ni cosas de él, y los agentes limitados a algunas facetas no lo ven. la búsqueda y la app del cuerpo sí lo muestran.

journal.window.stored.body.summary.strava.facet.default = los entrenamientos nuevos se archivan bajo la faceta { $facet }, la que elegiste para las importaciones de Strava.

journal.window.stored.body.summary.strava.facet.unset = los entrenamientos nuevos no tienen faceta, porque no se eligió ninguna para esta importación y no hay ninguna configurada para las importaciones de Strava, o la configurada está silenciada o ya no está: un entrenamiento sin faceta no aparece en la página de actividades, no salen personas ni cosas de él, y los agentes limitados a algunas facetas no lo ven. la búsqueda y la app del cuerpo sí lo muestran.

journal.window.stored.body.summary.strava.filed =

    { $quantity ->
        [one] 1 entrenamiento ahora está archivado bajo { $facet }.
        [many] { $count } entrenamientos ahora están archivados bajo { $facet }.
       *[other] { $count } entrenamientos ahora están archivados bajo { $facet }.
    }

journal.window.stored.body.summary.strava.filed.one.moved = 1 entrenamiento ahora está archivado bajo { $facet }. se movió desde otra faceta.

journal.window.stored.body.summary.strava.filed.some.moved =

    { $quantity ->
        [one] 1 entrenamiento ahora está archivado bajo { $facet }. { $moved } de ellos se movió desde otra faceta.
        [many] { $count } entrenamientos ahora están archivados bajo { $facet }. { $moved } de ellos se movieron desde otra faceta.
       *[other] { $count } entrenamientos ahora están archivados bajo { $facet }. { $moved } de ellos se movieron desde otra faceta.
    }

journal.window.stored.body.summary.strava.kept.filed =

    { $quantity ->
        [one] 1 entrenamiento se quedó bajo { $facet }, donde ya estaba archivado.
        [many] { $count } entrenamientos se quedaron bajo { $facet }, donde ya estaban archivados.
       *[other] { $count } entrenamientos se quedaron bajo { $facet }, donde ya estaban archivados.
    }

journal.window.stored.body.summary.strava.kept.sensed =

    { $quantity ->
        [one] 1 entrenamiento se dejó donde estaba, porque ya aparece en la página de actividades.
        [many] { $count } entrenamientos se dejaron donde estaban, porque ya aparecen en la página de actividades.
       *[other] { $count } entrenamientos se dejaron donde estaban, porque ya aparecen en la página de actividades.
    }

journal.window.stored.body.summary.strava.kept.unreadable =

    { $quantity ->
        [one] 1 entrenamiento se dejó donde estaba, porque tu diario no pudo leer lo que aparece en la página de actividades de su día. volver a importar intenta una vez más.
        [many] { $count } entrenamientos se dejaron donde estaban, porque tu diario no pudo leer lo que aparece en la página de actividades de sus días. volver a importar intenta una vez más.
       *[other] { $count } entrenamientos se dejaron donde estaban, porque tu diario no pudo leer lo que aparece en la página de actividades de sus días. volver a importar intenta una vez más.
    }

journal.window.stored.body.summary.strava.listed.moved =

    { $quantity ->
        [one] 1 entrenamiento pasó a la faceta de esta importación, desde { $facets }.
        [many] { $count } entrenamientos pasaron a la faceta de esta importación, desde { $facets }.
       *[other] { $count } entrenamientos pasaron a la faceta de esta importación, desde { $facets }.
    }

journal.window.stored.body.summary.strava.listed.over.none.shown =

    { $quantity ->
        [one] nada de lo ya archivado se movió: se habría movido 1 entrenamiento, y no se te mostró ninguno.
        [many] nada de lo ya archivado se movió: se habrían movido { $count } entrenamientos, y no se te mostró ninguno.
       *[other] nada de lo ya archivado se movió: se habrían movido { $count } entrenamientos, y no se te mostró ninguno.
    }

journal.window.stored.body.summary.strava.listed.over.shown =

    { $quantity ->
        [one] nada de lo ya archivado se movió: se habría movido 1 entrenamiento, y se te dijo { $shown }.
        [many] nada de lo ya archivado se movió: se habrían movido { $count } entrenamientos, y se te dijo { $shown }.
       *[other] nada de lo ya archivado se movió: se habrían movido { $count } entrenamientos, y se te dijo { $shown }.
    }

journal.window.stored.body.summary.strava.names.cached =

    { $quantity ->
        [one] las palabras de 1 entrenamiento se leyeron en una importación anterior.
        [many] las palabras de { $count } entrenamientos se leyeron en una importación anterior.
       *[other] las palabras de { $count } entrenamientos se leyeron en una importación anterior.
    }

journal.window.stored.body.summary.strava.names.created =

    { $quantity ->
        [one] se agregó 1 persona o cosa a partir de tus entrenamientos.
        [many] se agregaron { $count } personas y cosas a partir de tus entrenamientos.
       *[other] se agregaron { $count } personas y cosas a partir de tus entrenamientos.
    }

journal.window.stored.body.summary.strava.names.elsewhere =

    { $quantity ->
        [one] no se leyeron las palabras de 1 entrenamiento, porque otra importación está leyendo las palabras de los entrenamientos ahora mismo. se leen cuando vuelves a importar.
        [many] no se leyeron las palabras de { $count } entrenamientos, porque otra importación está leyendo las palabras de los entrenamientos ahora mismo. se leen cuando vuelves a importar.
       *[other] no se leyeron las palabras de { $count } entrenamientos, porque otra importación está leyendo las palabras de los entrenamientos ahora mismo. se leen cuando vuelves a importar.
    }

journal.window.stored.body.summary.strava.names.failed =

    { $quantity ->
        [one] no se pudo leer 1 entrenamiento, y volver a importar intenta una vez más.
        [many] no se pudieron leer { $count } entrenamientos, y volver a importar intenta una vez más.
       *[other] no se pudieron leer { $count } entrenamientos, y volver a importar intenta una vez más.
    }

journal.window.stored.body.summary.strava.names.none.created = esta vez no se agregaron personas ni cosas.

journal.window.stored.body.summary.strava.names.promotion.unavailable = esta vez no se pudieron agregar personas ni cosas, porque tu diario no pudo leer tus facetas. volver a importar intenta una vez más.

journal.window.stored.body.summary.strava.names.read =

    { $quantity ->
        [one] se leyeron las propias palabras de 1 entrenamiento para las personas y el equipo que nombran.
        [many] se leyeron las propias palabras de { $count } entrenamientos para las personas y el equipo que nombran.
       *[other] se leyeron las propias palabras de { $count } entrenamientos para las personas y el equipo que nombran.
    }

journal.window.stored.body.summary.strava.names.set.aside =

    { $quantity ->
        [one] no se leyeron las palabras de 1 entrenamiento, porque está archivado bajo una faceta silenciada o que ya no está. se leen cuando esa faceta vuelve a estar activada, o una importación las archiva bajo otra.
        [many] no se leyeron las palabras de { $count } entrenamientos, porque están archivados bajo una faceta silenciada o que ya no está. se leen cuando esa faceta vuelve a estar activada, o una importación las archiva bajo otra.
       *[other] no se leyeron las palabras de { $count } entrenamientos, porque están archivados bajo una faceta silenciada o que ya no está. se leen cuando esa faceta vuelve a estar activada, o una importación las archiva bajo otra.
    }

journal.window.stored.body.summary.strava.names.store.unavailable = tu diario no pudo abrir o guardar lo que anotó de tus entrenamientos, así que esta vez no toda persona o pieza de equipo de ellos se leyó o se agregó. volver a importar intenta una vez más.

journal.window.stored.body.summary.strava.names.waiting.lane =

    { $quantity ->
        [one] esta vez no se leyeron las palabras de 1 entrenamiento: no hay un razonamiento configurado que pueda leerlas, así que se leen cuando lo esté y vuelvas a importar.
        [many] esta vez no se leyeron las palabras de { $count } entrenamientos: no hay un razonamiento configurado que pueda leerlas, así que se leen cuando lo esté y vuelvas a importar.
       *[other] esta vez no se leyeron las palabras de { $count } entrenamientos: no hay un razonamiento configurado que pueda leerlas, así que se leen cuando lo esté y vuelvas a importar.
    }

journal.window.stored.body.summary.strava.names.waiting.no.condition =

    { $quantity ->
        [one] esta vez no se leyeron las palabras de 1 entrenamiento: tu diario espera una ventana de procesamiento, y no hay ninguna configurada, así que se leen cuando haya una y vuelvas a importar.
        [many] esta vez no se leyeron las palabras de { $count } entrenamientos: tu diario espera una ventana de procesamiento, y no hay ninguna configurada, así que se leen cuando haya una y vuelvas a importar.
       *[other] esta vez no se leyeron las palabras de { $count } entrenamientos: tu diario espera una ventana de procesamiento, y no hay ninguna configurada, así que se leen cuando haya una y vuelvas a importar.
    }

journal.window.stored.body.summary.strava.names.waiting.settings =

    { $quantity ->
        [one] esta vez no se leyeron las palabras de 1 entrenamiento: no se pudo leer la configuración de tu diario, así que se leen cuando vuelves a importar.
        [many] esta vez no se leyeron las palabras de { $count } entrenamientos: no se pudo leer la configuración de tu diario, así que se leen cuando vuelves a importar.
       *[other] esta vez no se leyeron las palabras de { $count } entrenamientos: no se pudo leer la configuración de tu diario, así que se leen cuando vuelves a importar.
    }

journal.window.stored.body.summary.strava.names.waiting.stopped =

    { $quantity ->
        [one] esta vez no se leyeron las palabras de 1 entrenamiento: se leen cuando vuelves a importar.
        [many] esta vez no se leyeron las palabras de { $count } entrenamientos: se leen cuando vuelves a importar.
       *[other] esta vez no se leyeron las palabras de { $count } entrenamientos: se leen cuando vuelves a importar.
    }

journal.window.stored.body.summary.strava.names.waiting.window =

    { $quantity ->
        [one] esta vez no se leyeron las palabras de 1 entrenamiento: se leen cuando importas durante tu ventana de procesamiento.
        [many] esta vez no se leyeron las palabras de { $count } entrenamientos: se leen cuando importas durante tu ventana de procesamiento.
       *[other] esta vez no se leyeron las palabras de { $count } entrenamientos: se leen cuando importas durante tu ventana de procesamiento.
    }

journal.window.stored.body.summary.strava.records.no.facet =

    { $quantity ->
        [one] 1 entrenamiento no tiene faceta, así que no aparece en la página de actividades.
        [many] { $count } entrenamientos no tienen faceta, así que no aparecen en la página de actividades.
       *[other] { $count } entrenamientos no tienen faceta, así que no aparecen en la página de actividades.
    }

journal.window.stored.body.summary.strava.records.refused = esta vez no todos los entrenamientos se pudieron incluir en la página de actividades. volver a importar intenta una vez más.

journal.window.stored.body.summary.strava.records.written =

    { $quantity ->
        [one] 1 entrenamiento está recién incluido en la página de actividades de su día.
        [many] { $count } entrenamientos están recién incluidos en la página de actividades de sus días.
       *[other] { $count } entrenamientos están recién incluidos en la página de actividades de sus días.
    }

journal.window.stored.body.summary.strava.replaced =

    { $quantity ->
        [one] 1 entrenamiento se volvió a colocar en el momento correcto, donde una versión anterior de tu diario lo había puesto en el momento incorrecto.
        [many] { $count } entrenamientos se volvieron a colocar en el momento correcto, donde una versión anterior de tu diario los había puesto en el momento incorrecto.
       *[other] { $count } entrenamientos se volvieron a colocar en el momento correcto, donde una versión anterior de tu diario los había puesto en el momento incorrecto.
    }

journal.window.stored.body.summary.strava.section.activities = ## actividades

journal.window.stored.body.summary.strava.section.facet = ## faceta

journal.window.stored.body.summary.strava.section.people.and.gear = ## personas y equipo

journal.window.stored.body.summary.strava.updated.only =

    { $quantity ->
        [one] ningún entrenamiento nuevo, 1 entrenamiento cambió desde la última importación y quedó al día.
        [many] ningún entrenamiento nuevo, { $updated } entrenamientos cambiaron desde la última importación y quedaron al día.
       *[other] ningún entrenamiento nuevo, { $updated } entrenamientos cambiaron desde la última importación y quedaron al día.
    }

journal.window.stored.body.summary.took.facet =

    { $quantity ->
        [one] 1 parte de tu diario tomó la faceta de la entrada que Oura movió a ella, así que lo que hay en ella es legible bajo esa faceta.
        [many] { $count } partes de tu diario tomaron cada una la faceta de la entrada que Oura movió a ella, así que lo que hay en ellas es legible bajo esa faceta.
       *[other] { $count } partes de tu diario tomaron cada una la faceta de la entrada que Oura movió a ella, así que lo que hay en ellas es legible bajo esa faceta.
    }

journal.window.stored.body.summary.unsure.bullet =

    { $quantity ->
        [one] - { $endpoint }: 1 sin agregar; el diario no puede saber si estaba en lo que eliminaste de { $days }
        [many] - { $endpoint }: { $count } sin agregar; el diario no puede saber si estaban en lo que eliminaste de { $days }
       *[other] - { $endpoint }: { $count } sin agregar; el diario no puede saber si estaban en lo que eliminaste de { $days }
    }

journal.window.stored.body.summary.windows.count =

    { $quantity ->
        [one] 1 parte de tu diario
        [many] { $count } partes de tu diario
       *[other] { $count } partes de tu diario
    }

journal.window.stored.document.calls.count =
    { $quantity ->
    [one] 1 llamada al modelo
    [many] { $count } llamadas al modelo
   *[other] { $count } llamadas al modelo
    }

journal.window.stored.document.date = **Fecha:** { $date } ({ $source })

journal.window.stored.document.engine.unknown = desconocido

journal.window.stored.document.extraction = **Extracción:** { $engine } — { $text_layer } de capa de texto, { $model_extracted } extraídas por el modelo, { $unavailable } no disponibles de { $pages }; { $image_described } con imagen descrita; { $model_calls }

journal.window.stored.document.marker.image.description = > [descripción de la imagen — generada por el modelo]

journal.window.stored.document.marker.image.description.raster = > [descripción de la imagen — generada por el modelo; original: { $path }]

journal.window.stored.document.marker.image.description.unavailable.no.raster = > [descripción de la imagen no disponible — { $reason }; no se pudo producir una imagen de la página]

journal.window.stored.document.marker.image.description.unavailable.raster = > [descripción de la imagen no disponible — { $reason }; imagen de la página preservada en { $path }]

journal.window.stored.document.marker.image.description.unavailable.stripped = > [descripción de la imagen no disponible — { $reason }]

journal.window.stored.document.marker.model.extracted = > [extraído por el modelo de la imagen de la página — puede contener errores]

journal.window.stored.document.marker.model.extracted.raster = > [extraído por el modelo de la imagen de la página — puede contener errores; original: { $path }]

journal.window.stored.document.marker.page.text.unavailable.no.raster = > [texto de la página no disponible — { $reason }; no se pudo producir una imagen de la página]

journal.window.stored.document.marker.page.text.unavailable.raster = > [texto de la página no disponible — { $reason }; imagen de la página preservada en { $path }]

journal.window.stored.document.marker.page.text.unavailable.stripped = > [texto de la página no disponible — { $reason }]

journal.window.stored.document.page.heading = ## Página { $page }

journal.window.stored.document.pages = **Páginas:** { $count }

journal.window.stored.document.pages.count =
    { $quantity ->
    [one] 1 página
    [many] { $count } páginas
   *[other] { $count } páginas
    }

journal.window.stored.document.type = **Tipo:** Documento

journal.window.stored.document.worker.warnings = **Advertencias del extractor:**

journal.window.stored.event.all.day.multi =
    { $quantity ->
    [one] **{ $date }** ({ $days } día)
    [many] **{ $date }** ({ $days } días)
   *[other] **{ $date }** ({ $days } días)
    }

journal.window.stored.event.all.day.single = **{ $date }** (todo el día)

journal.window.stored.event.preview.all.day = todo el día

journal.window.stored.event.preview.days =
    { $quantity ->
    [one] { $days } día
    [many] { $days } días
   *[other] { $days } días
    }

journal.window.stored.event.repeats = Se repite: { $rule }

journal.window.stored.event.rrule.count =
    { $quantity ->
    [one] { $base }, 1 vez
    [many] { $base }, { $count } veces
   *[other] { $base }, { $count } veces
    }

journal.window.stored.event.rrule.daily = diariamente

journal.window.stored.event.rrule.every.days = Cada { $interval } días

journal.window.stored.event.rrule.every.months = Cada { $interval } meses

journal.window.stored.event.rrule.every.weeks = Cada { $interval } semanas

journal.window.stored.event.rrule.every.years = Cada { $interval } años

journal.window.stored.event.rrule.in.months = { $base } en { $months }

journal.window.stored.event.rrule.month.apr = abr

journal.window.stored.event.rrule.month.aug = ago

journal.window.stored.event.rrule.month.dec = dic

journal.window.stored.event.rrule.month.feb = feb

journal.window.stored.event.rrule.month.jan = ene

journal.window.stored.event.rrule.month.jul = jul

journal.window.stored.event.rrule.month.jun = jun

journal.window.stored.event.rrule.month.mar = mar

journal.window.stored.event.rrule.month.may = may

journal.window.stored.event.rrule.month.nov = nov

journal.window.stored.event.rrule.month.oct = oct

journal.window.stored.event.rrule.month.sep = sep

journal.window.stored.event.rrule.monthly = Mensual

journal.window.stored.event.rrule.on.days = { $base } los { $days }

journal.window.stored.event.rrule.on.monthday = { $base } el día { $days }

journal.window.stored.event.rrule.until = { $base }, hasta { $until }

journal.window.stored.event.rrule.weekday.fri = vie

journal.window.stored.event.rrule.weekday.mon = lun

journal.window.stored.event.rrule.weekday.sat = sáb

journal.window.stored.event.rrule.weekday.sun = dom

journal.window.stored.event.rrule.weekday.thu = jue

journal.window.stored.event.rrule.weekday.tue = mar

journal.window.stored.event.rrule.weekday.wed = mié

journal.window.stored.event.rrule.weekly = semanalmente

journal.window.stored.event.rrule.yearly = Anual

journal.window.stored.event.timed.duration = **{ $when }** ({ $minutes } min)

journal.window.stored.event.where = Dónde: { $location }

journal.window.stored.event.with = Con: { $names }

journal.window.stored.evernote.attachment.missing = falta un archivo adjunto en esta exportación.

journal.window.stored.evernote.attachment.oversized = un archivo adjunto pasa de 25 MB, así que se dejó fuera.

journal.window.stored.evernote.attachment.unmatched = un archivo adjunto no coincidió con esta nota, así que se dejó fuera.

journal.window.stored.evernote.author = autor: { $author }

journal.window.stored.evernote.encrypted = una parte de esta nota está cifrada en Evernote. no se trajo.

journal.window.stored.evernote.tags = etiquetas: { $tags }

journal.window.stored.evernote.task.due = { $title }, vence { $due }

journal.window.stored.evernote.url = url: { $url }

journal.window.stored.image.caption = **Leyenda:**

journal.window.stored.image.date = **Fecha:** { $date }

journal.window.stored.image.description.header = > [descripción de la imagen — derivada del modelo]

journal.window.stored.image.description.unavailable = > no disponible — { $reason }

journal.window.stored.image.dimensions = **Dimensiones:** { $width }×{ $height }

journal.window.stored.image.format = **Formato:** { $format }

journal.window.stored.image.type = **Tipo:** Imagen
