journal.cli.backup.config.canonical.recovery.characters = la clave de recuperación canónica contiene caracteres { $Crockford } no válidos

journal.cli.backup.config.canonical.recovery.length = la clave de recuperación canónica debe tener exactamente 64 caracteres

journal.cli.backup.config.entropy = no se pudo generar la clave de copia de seguridad.

journal.cli.backup.config.hosted.delete = no se pudo eliminar el enlace alojado.

journal.cli.backup.config.hosted.read = no se pudo leer el enlace alojado.

journal.cli.backup.config.hosted.write = no se pudo guardar el enlace alojado.

journal.cli.backup.config.invalid.destination.shape = el destino de copia de seguridad debe ser un objeto { $JSON }

journal.cli.backup.config.invalid.mode = el modo de copia de seguridad debe ser { $byo } o { $operated }

journal.cli.backup.config.invalid.offload.bytes = los valores de bytes del traslado de copia de seguridad deben ser enteros positivos o null

journal.cli.backup.config.invalid.offload.enabled = el enabled del traslado de copia de seguridad debe ser un booleano

journal.cli.backup.config.invalid.offload.shape = el traslado de copia de seguridad debe incluir { $enabled }, { $budget_bytes }, { $floor_bytes }

journal.cli.backup.config.invalid.offload.status = el estado del traslado de copia de seguridad debe ser { $ok_value }, { $skipped_value }, { $stalled_value } o { $error_value }

journal.cli.backup.config.invalid.restore.counters = los contadores de restauración de copia de seguridad deben ser enteros no negativos

journal.cli.backup.config.invalid.restore.scope = el alcance de la restauración de copia de seguridad debe ser { $day_value }, { $all_value } o { $journal_value }

journal.cli.backup.config.invalid.restore.status = el estado de la restauración de copia de seguridad debe ser { $ok_value }, { $no_op_value }, { $refused_value }, { $degraded_value } o { $error_value }

journal.cli.backup.config.invalid.retention.shape = la retención de copia de seguridad debe incluir { $hourly }, { $daily }, { $weekly }, { $monthly }

journal.cli.backup.config.invalid.retention.value = los valores de retención de copia de seguridad deben ser enteros no negativos

journal.cli.backup.config.invalid.verification.status = el estado de la verificación de copia de seguridad debe ser { $ok_value }, { $skipped_value } o { $error_value }

journal.cli.backup.config.lock.timeout = no se pudo adquirir el bloqueo para { $path } dentro de { $seconds }s

journal.cli.backup.config.missing.credential = falta la credencial de copia de seguridad: { $key }

journal.cli.backup.config.recovery.parse = la clave de recuperación debe contener exactamente 64 caracteres { $Crockford } después de la limpieza

journal.cli.backup.config.stored.keys = las claves de copia de seguridad deben ser cadenas cuando existen

journal.cli.backup.config.teardown.recovery.corrupt = registro de recuperación del desmontaje corrupto o no válido.

journal.cli.backup.config.teardown.recovery.delete = no se pudo eliminar el registro de recuperación del desmontaje.

journal.cli.backup.config.teardown.recovery.read = no se pudo leer el registro de recuperación del desmontaje.

journal.cli.backup.config.teardown.recovery.write = no se pudo guardar el registro de recuperación del desmontaje.

journal.cli.backup.config.unsupported.backend = backend de copia de seguridad no admitido

journal.cli.backup.destination.backend.prefix = El repositorio debe empezar con { $backend }: (por ejemplo { $example }).

journal.cli.backup.destination.hosted = { $company } opera el almacenamiento de tu copia de seguridad, así que no hay un destino que cambiar aquí. adminístralo en el portal de servicios.

journal.cli.backup.destination.refused.auth.failed = el destino rechazó las credenciales. no se guardó nada.

journal.cli.backup.destination.refused.bucket.not.found = estas credenciales no pueden abrir un bucket con ese nombre. revisa el nombre del bucket en el repositorio. no se guardó nada.

journal.cli.backup.destination.refused.locked = la copia de seguridad de ahí está ocupada. vuelve a intentarlo en breve. no se guardó nada.

journal.cli.backup.destination.refused.other = no se pudo abrir el destino. revisa el repositorio y las credenciales, luego vuelve a intentarlo. no se guardó nada.

journal.cli.backup.destination.refused.repo.missing = no se encontró una copia de seguridad ahí con estas credenciales. no se guardó nada.

journal.cli.backup.destination.refused.repository.key.refused = ya hay una copia de seguridad ahí que tu clave de recuperación no puede abrir. elige otro destino. no se guardó nada.

journal.cli.backup.destination.refused.timeout = el destino no respondió a tiempo. revisa el repositorio y las credenciales, luego vuelve a intentarlo. no se guardó nada.

journal.cli.backup.destination.setup.failed = no se pudo configurar una copia de seguridad ahí ({ $reason }). no se guardó nada.

journal.cli.backup.destination.switch.confirm = para cambiar, vuelve a ejecutar con { $confirm_new_repository_flag }.

journal.cli.backup.destination.switch.lead = ese es un repositorio distinto. tu copia de seguridad existente se queda donde está, intacta, y tu diario no hará copias de seguridad en él ni restaurará desde él.

journal.cli.backup.enable.complete = Copia de seguridad activada.

journal.cli.backup.enable.complete.key = Copia de seguridad activada. Tu clave de recuperación:{ $line_break }{ $key_grid }

journal.cli.backup.enable.confirm.key = Tu clave de recuperación (anótala - es la única forma de restaurar):{ $line_break }{ $key_grid }{ $line_break }Confirma canalizando la clave de vuelta: { $command_enable }

journal.cli.backup.enable.destination.required = Establece un destino primero: { $command_destination_set }

journal.cli.backup.enable.key.mismatch = La clave de recuperación no coincidió.

journal.cli.backup.enable.repository.missing = Repositorio no encontrado; preinicialízalo o establece { $recovery_setting } en la configuración.

journal.cli.backup.error = Error: { $message }

journal.cli.backup.help.destination = uso: { $syntax }

journal.cli.backup.help.offload = uso: { $syntax }

journal.cli.backup.help.recovery.key = uso: { $syntax }

journal.cli.backup.help.root = uso: { $syntax }

journal.cli.backup.hosted.field = Falta { $name }.

journal.cli.backup.init.code = Falló la inicialización del repositorio ({ $code_field }{ $returncode }).

journal.cli.backup.init.failed = falló la inicialización del repositorio.

journal.cli.backup.init.retained = falló la inicialización del repositorio; el estado de la operación conservado requiere revisión.

journal.cli.backup.input.io = { $stdin_stream } error de E/S.

journal.cli.backup.input.json = { $json_format } no válido en { $stdin_stream }: { $error }

journal.cli.backup.input.limit = la solicitud de { $stdin_stream } supera el límite de entrada de { $json_format }.

journal.cli.backup.input.object = se esperaba un objeto { $json_format } en { $stdin_stream }.

journal.cli.backup.offload.media.byte.overflow = desbordamiento del total de bytes de multimedia

journal.cli.backup.offload.media.file.overflow = desbordamiento del total de archivos de multimedia

journal.cli.backup.offload.preview.held = una vista previa de solo lectura del traslado no está disponible. no se inició un trabajo de traslado.

journal.cli.backup.offload.restore.day.invalid = Día no válido.

journal.cli.backup.offload.restore.day.or.all = Usa un día o { $all_flag }, no ambos.

journal.cli.backup.offload.restore.day.required = Proporciona un día o { $all_flag }.

journal.cli.backup.offload.restore.heading = restauración del traslado de copia de seguridad:

journal.cli.backup.offload.restore.reason.archive.unbound = el registro de copia de seguridad no identifica el archivo comprimido necesario para restaurar estos archivos

journal.cli.backup.offload.restore.reason.backup.not.ready = tu copia de seguridad no está configurada, así que no se pueden recuperar los archivos trasladados

journal.cli.backup.offload.restore.reason.destination.admission.failed = no se pudo alcanzar el destino de la copia de seguridad

journal.cli.backup.offload.restore.reason.failed = el intento de restauración falló por un motivo no especificado

journal.cli.backup.offload.restore.reason.insufficient.free.space = no hay suficiente espacio libre en esta computadora para traer los archivos de vuelta

journal.cli.backup.offload.restore.reason.ledger.degraded = el registro de lo trasladado está dañado y no se puede usar para restaurar

journal.cli.backup.offload.restore.reason.legacy.read.failed = no se pudo leer la copia de seguridad del multimedia trasladado por una versión anterior de tu diario, así que no se restauró

journal.cli.backup.offload.restore.reason.legacy.review.held = no se pudo revisar el multimedia trasladado por una versión anterior de tu diario contra tu copia de seguridad, así que no se restauró

journal.cli.backup.offload.restore.reason.legacy.verification.failed = la copia de seguridad del multimedia trasladado por una versión anterior de tu diario no coincidió con su registro, así que no se restauró

journal.cli.backup.offload.restore.reason.locked = la copia de seguridad en el destino está bloqueada, normalmente por una ejecución que sigue en curso o por una que se interrumpió

journal.cli.backup.offload.restore.reason.missing.file.after.restore = falta un archivo que debía volver con la restauración

journal.cli.backup.offload.restore.reason.nothing.to.restore = no había nada trasladado que restaurar

journal.cli.backup.offload.restore.reason.rclone.unavailable = el programa { $rclone_program } no está disponible

journal.cli.backup.offload.restore.reason.repo.missing = todavía no hay una copia de seguridad en el destino

journal.cli.backup.offload.restore.reason.restic.unavailable = el programa de copia de seguridad { $restic_program } no está disponible

journal.cli.backup.offload.restore.reason.restore.conflict = ya hay un archivo distinto en el lugar que le corresponde a al menos uno de estos archivos, así que se conservó el que ya estaba y no se restauró la copia de seguridad

journal.cli.backup.offload.restore.reason.restore.held = la restauración de los archivos trasladados no está disponible temporalmente

journal.cli.backup.offload.restore.reason.restore.not.published = esta vez no se pudo volver a poner al menos un archivo en tu diario y no se puso nada en su lugar, así que vuelve a intentarlo más tarde

journal.cli.backup.offload.restore.reason.segment.identity = no se pudo verificar una de las grabaciones de tu diario contra su propio registro de identidad

journal.cli.backup.offload.restore.reason.segment.missing = la ubicación del diario a la que pertenecen estos archivos ya no existe en esta computadora

journal.cli.backup.offload.restore.reason.stream.marker.failed = los archivos se restauraron, pero después falló la actualización de los registros del diario

journal.cli.backup.offload.restore.reason.timeout = se ejecutó demasiado tiempo y se detuvo

journal.cli.backup.offload.restore.reason.unknown.code = se detuvo sin un motivo ({ $code })

journal.cli.backup.offload.restore.reason.verification.failed = un archivo restaurado no coincidió con lo que debía ser, así que puede estar dañado

journal.cli.backup.offload.restore.unfinished = la restauración no terminó limpiamente, así que vuelve a intentarlo más tarde.

journal.cli.backup.offload.status.heading = traslado de copia de seguridad:

journal.cli.backup.offload.status.invalid = estado de traslado no válido.

journal.cli.backup.offload.status.overflow = desbordamiento del total de estado de multimedia

journal.cli.backup.offload.use.installed = ejecuta `{ $command_offload_run }` con el comando { $program } instalado.

journal.cli.backup.output.json = no se pudo representar la salida { $json_format }.

journal.cli.backup.pause.already = la copia de seguridad ya está en pausa.

journal.cli.backup.pause.complete = copia de seguridad en pausa. ejecuta `{ $command_resume }` para iniciarla de nuevo.

journal.cli.backup.pause.off = la copia de seguridad está desactivada, así que no hay nada que pausar.

journal.cli.backup.payload.backend = Falta el backend.

journal.cli.backup.payload.credential = Falta la credencial: { $key }.

journal.cli.backup.payload.credentials = Faltan las credenciales.

journal.cli.backup.payload.recovery.key = Falta { $key_field }.

journal.cli.backup.payload.repository = Falta el repositorio.

journal.cli.backup.payload.unsupported = Backend no admitido.

journal.cli.backup.prune.complete = Recorte de retención completado.

journal.cli.backup.prune.detail.failure.rclone.unavailable = Falló el recorte: el programa { $rclone_program } no está disponible. { $detail_field }{ $detail }

journal.cli.backup.prune.detail.failure.restic.unavailable = Falló el recorte: el programa de copia de seguridad { $restic_program } no está disponible. { $detail_field }{ $detail }

journal.cli.backup.prune.failure.access = Falló el recorte: el destino denegó el permiso para esta escritura.

journal.cli.backup.prune.failure.auth.failed = Falló el recorte: el destino rechazó las credenciales.

journal.cli.backup.prune.failure.binding.invalid = Falló el recorte: la configuración de copia de seguridad de tu diario ya no es válida.

journal.cli.backup.prune.failure.binding.superseded = Falló el recorte: una configuración de copia de seguridad más nueva la reemplazó.

journal.cli.backup.prune.failure.broker.error = Falló el recorte: el servicio de copia de seguridad que { $company } opera no pudo completar la solicitud.

journal.cli.backup.prune.failure.broker.unreachable = Falló el recorte: no se pudo alcanzar el servicio de copia de seguridad que { $company } opera.

journal.cli.backup.prune.failure.bucket.not.found = Falló el recorte: tus credenciales no pueden abrir el bucket que nombra el repositorio.

journal.cli.backup.prune.failure.failed = Falló el recorte: no se dio un motivo.

journal.cli.backup.prune.failure.hosted.entitlement.inactive = Falló el recorte: { $company } no está operando la copia de seguridad para ti.

journal.cli.backup.prune.failure.journal.path.unresolved = Falló el recorte: no se pudo encontrar tu diario.

journal.cli.backup.prune.failure.locked = Falló el recorte: la copia de seguridad en el destino está bloqueada, normalmente por una ejecución que sigue en curso o por una que se interrumpió.

journal.cli.backup.prune.failure.rclone.unavailable = Falló el recorte: el programa { $rclone_program } no está disponible.

journal.cli.backup.prune.failure.repo.exists = Falló el recorte: ya hay una copia de seguridad ahí.

journal.cli.backup.prune.failure.repo.missing = Falló el recorte: todavía no hay una copia de seguridad en el destino.

journal.cli.backup.prune.failure.repository.unsupported = Falló el recorte: el repositorio en tu configuración de copia de seguridad no empieza con { $s3_backend } ni con { $b2_backend } para coincidir con su backend.

journal.cli.backup.prune.failure.restic.unavailable = Falló el recorte: el programa de copia de seguridad { $restic_program } no está disponible.

journal.cli.backup.prune.failure.timeout = Falló el recorte: se ejecutó demasiado tiempo y se detuvo.

journal.cli.backup.prune.failure.unknown.code = El recorte falló: se detuvo sin un motivo ({ $code }).

journal.cli.backup.prune.failure.unreachable = El recorte falló: no se pudo alcanzar el destino.

journal.cli.backup.prune.skipped = Recorte omitido (no activado o no configurado).

journal.cli.backup.recovery.key.missing = No hay una clave de recuperación configurada.

journal.cli.backup.restore.already.set.up = no se pudo iniciar la restauración: tu diario ya está configurado, así que no se puede restaurar encima. para usar una copia de seguridad, inicia otro diario y elige restaurar en la configuración inicial.

journal.cli.backup.restore.complete = restauración completa: { $counters }, { $integrity_field }{ $integrity }, { $resume_field }{ $resumable }.

journal.cli.backup.restore.degraded.integrity.failed = la restauración terminó con un problema: { $counters }; la copia de seguridad falló su revisión de integridad y puede estar dañada, así que tu diario no está establecido en la copia de seguridad ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.integrity.unverified = la restauración terminó con un problema: { $counters }; no se pudo ejecutar la revisión de integridad (la copia de seguridad estaba ocupada o se agotó el tiempo), así que tu diario no está establecido en la copia de seguridad ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.other = la restauración terminó con un problema: { $counters }; la restauración se completó con evidencia degradada ({ $reason_field }{ $code }).

journal.cli.backup.restore.degraded.restore.summary.missing = la restauración terminó con un problema: { $counters }; el resumen de la restauración faltaba o estaba mal formado ({ $reason_field }{ $code }).

journal.cli.backup.restore.failed.counters = la restauración falló: { $counters }.

journal.cli.backup.restore.failure.body.rebuild.failed = la restauración falló: no se pudo reconstruir tu diario a partir de los archivos restaurados; { $counters }.

journal.cli.backup.restore.failure.destination.admission.failed = la restauración falló: no se pudo alcanzar el destino de la copia de seguridad; { $counters }.

journal.cli.backup.restore.failure.destination.invalid = la restauración falló: el destino de la copia de seguridad no es válido; { $counters }.

journal.cli.backup.restore.failure.destination.publish.failed = la restauración falló: no se pudo guardar el diario restaurado en su destino; { $counters }.

journal.cli.backup.restore.failure.full.scan.failed = la restauración falló: un escaneo completo del diario restaurado falló; { $counters }.

journal.cli.backup.restore.failure.insufficient.free.space = la restauración falló: no hay suficiente espacio libre para esta restauración, así que no cambió nada; { $counters }.

journal.cli.backup.restore.failure.integrity.check.io.failed = la restauración falló: no se pudieron revisar los archivos restaurados; { $counters }.

journal.cli.backup.restore.failure.integrity.failed = la restauración falló: la copia de seguridad falló su revisión de integridad y puede estar dañada, así que tu diario no está establecido en la copia de seguridad; { $counters }.

journal.cli.backup.restore.failure.integrity.unverified = la restauración falló: no se pudo ejecutar la revisión de integridad (la copia de seguridad estaba ocupada o se agotó el tiempo), así que tu diario no está establecido en la copia de seguridad; { $counters }.

journal.cli.backup.restore.failure.invalid.key = la restauración falló: la clave de recuperación ingresada no coincide con esta copia de seguridad; { $counters }.

journal.cli.backup.restore.failure.journal.already.set.up = la restauración falló: tu diario ya está configurado, así que no se puede restaurar encima. para usar una copia de seguridad, inicia otro diario y elige restaurar en la configuración inicial; { $counters }.

journal.cli.backup.restore.failure.journal.snapshot.not.found = la restauración falló: no se encontró una copia de seguridad de tu diario; { $counters }.

journal.cli.backup.restore.failure.recovery.confirmation.publish.failed = la restauración falló: no se pudo guardar la confirmación de recuperación; { $counters }.

journal.cli.backup.restore.failure.recovery.key.publish.failed = la restauración falló: no se pudo guardar la clave de recuperación; { $counters }.

journal.cli.backup.restore.failure.restic.unavailable = la restauración falló: el programa de copia de seguridad { $restic_program } no está disponible; { $counters }.

journal.cli.backup.restore.failure.restore.failed = la restauración falló: la restauración no se completó; { $counters }.

journal.cli.backup.restore.failure.restore.io.failed = la restauración falló: no se pudieron escribir archivos durante la restauración; { $counters }.

journal.cli.backup.restore.failure.restore.record.failed = la restauración falló: no se pudo registrar el resultado de la restauración; { $counters }.

journal.cli.backup.restore.failure.restore.summary.missing = la restauración falló: la restauración terminó, pero falta su resumen; { $counters }.

journal.cli.backup.restore.failure.snapshot.catalog.invalid = la restauración falló: la lista de copias de seguridad no es válida; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.failed = la restauración falló: no se pudo obtener la lista de copias de seguridad; { $counters }.

journal.cli.backup.restore.failure.snapshot.list.io.failed = la restauración falló: no se pudo leer la lista de copias de seguridad; { $counters }.

journal.cli.backup.restore.failure.snapshot.selection.ambiguous = la restauración falló: más de una copia de seguridad coincidió y no se pudo elegir ninguna automáticamente; { $counters }.

journal.cli.backup.restore.failure.unknown.code = la restauración falló: se detuvo sin un motivo ({ $code }); { $counters }.

journal.cli.backup.restore.recording.failure.body.rebuild.failed = También falló el registro del resultado: no se pudo reconstruir tu diario a partir de los archivos restaurados.

journal.cli.backup.restore.recording.failure.destination.admission.failed = También falló el registro del resultado: no se pudo alcanzar el destino de la copia de seguridad.

journal.cli.backup.restore.recording.failure.destination.invalid = También falló el registro del resultado: el destino de la copia de seguridad no es válido.

journal.cli.backup.restore.recording.failure.destination.publish.failed = También falló el registro del resultado: no se pudo guardar el diario restaurado en su destino.

journal.cli.backup.restore.recording.failure.full.scan.failed = También falló el registro del resultado: un escaneo completo del diario restaurado falló.

journal.cli.backup.restore.recording.failure.insufficient.free.space = También falló el registro del resultado: no hay suficiente espacio libre para esta restauración, así que no cambió nada.

journal.cli.backup.restore.recording.failure.integrity.check.io.failed = También falló el registro del resultado: no se pudieron revisar los archivos restaurados.

journal.cli.backup.restore.recording.failure.integrity.failed = También falló el registro del resultado: la copia de seguridad falló su revisión de integridad y puede estar dañada, así que tu diario no está establecido en la copia de seguridad.

journal.cli.backup.restore.recording.failure.integrity.unverified = También falló el registro del resultado: no se pudo ejecutar la revisión de integridad (la copia de seguridad estaba ocupada o se agotó el tiempo), así que tu diario no está establecido en la copia de seguridad.

journal.cli.backup.restore.recording.failure.invalid.key = También falló el registro del resultado: la clave de recuperación ingresada no coincide con esta copia de seguridad.

journal.cli.backup.restore.recording.failure.journal.already.set.up = También falló el registro del resultado: tu diario ya está configurado, así que no se puede restaurar encima. para usar una copia de seguridad, inicia otro diario y elige restaurar en la configuración inicial.

journal.cli.backup.restore.recording.failure.journal.snapshot.not.found = También falló el registro del resultado: no se encontró una copia de seguridad de tu diario.

journal.cli.backup.restore.recording.failure.recovery.confirmation.publish.failed = También falló el registro del resultado: no se pudo guardar la confirmación de recuperación.

journal.cli.backup.restore.recording.failure.recovery.key.publish.failed = También falló el registro del resultado: no se pudo guardar la clave de recuperación.

journal.cli.backup.restore.recording.failure.restic.unavailable = También falló el registro del resultado: el programa de copia de seguridad { $restic_program } no está disponible.

journal.cli.backup.restore.recording.failure.restore.failed = También falló el registro del resultado: la restauración no se completó.

journal.cli.backup.restore.recording.failure.restore.io.failed = También falló el registro del resultado: no se pudieron escribir archivos durante la restauración.

journal.cli.backup.restore.recording.failure.restore.record.failed = También falló el registro del resultado: no se pudo registrar el resultado de la restauración.

journal.cli.backup.restore.recording.failure.restore.summary.missing = También falló el registro del resultado: la restauración terminó, pero falta su resumen.

journal.cli.backup.restore.recording.failure.snapshot.catalog.invalid = También falló el registro del resultado: la lista de copias de seguridad no es válida.

journal.cli.backup.restore.recording.failure.snapshot.list.failed = También falló el registro del resultado: no se pudo obtener la lista de copias de seguridad.

journal.cli.backup.restore.recording.failure.snapshot.list.io.failed = También falló el registro del resultado: no se pudo leer la lista de copias de seguridad.

journal.cli.backup.restore.recording.failure.snapshot.selection.ambiguous = También falló el registro del resultado: más de una copia de seguridad coincidió y no se pudo elegir ninguna automáticamente.

journal.cli.backup.restore.recording.failure.unknown.code = También falló el registro del resultado: se detuvo sin un motivo ({ $code }).

journal.cli.backup.restore.unrecorded = la restauración se completó, pero falló el registro del resultado: { $counters } ({ $reason_field }{ $restore_record }).

journal.cli.backup.resume.complete = se reanudó la copia de seguridad. la próxima se ejecuta en su horario habitual.

journal.cli.backup.resume.not.paused = la copia de seguridad no está en pausa.

journal.cli.backup.rotation.failed = La rotación falló: falló.

journal.cli.backup.rotation.failure.access = La rotación falló: el destino denegó el permiso para esta escritura.

journal.cli.backup.rotation.failure.auth.failed = La rotación falló: el destino rechazó las credenciales.

journal.cli.backup.rotation.failure.binding.invalid = La rotación falló: la configuración de copia de seguridad de tu diario ya no es válida.

journal.cli.backup.rotation.failure.binding.superseded = La rotación falló: configuración de copia de seguridad más reciente reemplazó esta.

journal.cli.backup.rotation.failure.broker.error = La rotación falló: el servicio de copia de seguridad que { $company } ejecuta no pudo completar la solicitud.

journal.cli.backup.rotation.failure.broker.unreachable = La rotación falló: no se pudo alcanzar el servicio de copia de seguridad que { $company } ejecuta.

journal.cli.backup.rotation.failure.bucket.not.found = La rotación falló: tus credenciales no pueden abrir el bucket que nombra el repositorio.

journal.cli.backup.rotation.failure.failed = La rotación falló: no se dio un motivo.

journal.cli.backup.rotation.failure.hosted.entitlement.inactive = La rotación falló: { $company } no está ejecutando la copia de seguridad para ti.

journal.cli.backup.rotation.failure.journal.path.unresolved = La rotación falló: no se pudo encontrar tu diario.

journal.cli.backup.rotation.failure.locked = La rotación falló: la copia de seguridad en el destino está bloqueada, por lo general por una ejecución en curso o una interrumpida.

journal.cli.backup.rotation.failure.rclone.unavailable = La rotación falló: el programa { $rclone_program } no está disponible.

journal.cli.backup.rotation.failure.repo.exists = La rotación falló: ya hay una copia de seguridad ahí.

journal.cli.backup.rotation.failure.repo.missing = La rotación falló: todavía no hay una copia de seguridad en el destino.

journal.cli.backup.rotation.failure.repository.unsupported = La rotación falló: el repositorio en tu configuración de copia de seguridad no empieza ni con { $s3_backend } ni con { $b2_backend } para coincidir con su backend.

journal.cli.backup.rotation.failure.restic.unavailable = La rotación falló: el programa de copia de seguridad { $restic_program } no está disponible.

journal.cli.backup.rotation.failure.timeout = La rotación falló: se ejecutó demasiado tiempo y se detuvo.

journal.cli.backup.rotation.failure.unknown.code = La rotación falló: se detuvo sin un motivo ({ $code }).

journal.cli.backup.rotation.failure.unreachable = La rotación falló: no se pudo alcanzar el destino.

journal.cli.backup.rotation.key = Nueva clave de recuperación (anótala):{ $line_break }{ $key_grid }

journal.cli.backup.rotation.skipped = Se omitió la rotación de la clave de recuperación (copia de seguridad no configurada).

journal.cli.backup.run.complete = Copia de seguridad completa (instantánea { $snapshot }).

journal.cli.backup.run.failure.access = La copia de seguridad falló: el destino denegó el permiso para esta escritura.

journal.cli.backup.run.failure.auth.failed = La copia de seguridad falló: el destino rechazó las credenciales.

journal.cli.backup.run.failure.binding.invalid = La copia de seguridad falló: la configuración de copia de seguridad de tu diario ya no es válida.

journal.cli.backup.run.failure.binding.superseded = La copia de seguridad falló: configuración de copia de seguridad más reciente reemplazó esta.

journal.cli.backup.run.failure.broker.error = La copia de seguridad falló: el servicio de copia de seguridad que { $company } ejecuta no pudo completar la solicitud.

journal.cli.backup.run.failure.broker.unreachable = La copia de seguridad falló: no se pudo alcanzar el servicio de copia de seguridad que { $company } ejecuta.

journal.cli.backup.run.failure.bucket.not.found = La copia de seguridad falló: tus credenciales no pueden abrir el bucket que nombra el repositorio.

journal.cli.backup.run.failure.failed = La copia de seguridad falló: no se dio un motivo.

journal.cli.backup.run.failure.hosted.entitlement.inactive = La copia de seguridad falló: { $company } no está ejecutando la copia de seguridad para ti.

journal.cli.backup.run.failure.journal.path.unresolved = La copia de seguridad falló: no se pudo encontrar tu diario.

journal.cli.backup.run.failure.locked = La copia de seguridad falló: la copia de seguridad en el destino está bloqueada, por lo general por una ejecución en curso o una interrumpida.

journal.cli.backup.run.failure.rclone.unavailable = La copia de seguridad falló: el programa { $rclone_program } no está disponible.

journal.cli.backup.run.failure.repo.exists = La copia de seguridad falló: ya hay una copia de seguridad ahí.

journal.cli.backup.run.failure.repo.missing = La copia de seguridad falló: todavía no hay una copia de seguridad en el destino.

journal.cli.backup.run.failure.repository.unsupported = La copia de seguridad falló: el repositorio en tu configuración de copia de seguridad no empieza ni con { $s3_backend } ni con { $b2_backend } para coincidir con su backend.

journal.cli.backup.run.failure.restic.unavailable = La copia de seguridad falló: el programa de copia de seguridad { $restic_program } no está disponible.

journal.cli.backup.run.failure.timeout = La copia de seguridad falló: se ejecutó demasiado tiempo y se detuvo.

journal.cli.backup.run.failure.unknown.code = La copia de seguridad falló: se detuvo sin un motivo ({ $code }).

journal.cli.backup.run.failure.unreachable = La copia de seguridad falló: no se pudo alcanzar el destino.

journal.cli.backup.run.partial.snapshot.count =
    { $quantity ->
        [one] Copia de seguridad parcial (instantánea { $snapshot }). 1 archivo no se pudo leer, así que no está en ella. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
        [many] Copia de seguridad parcial (instantánea { $snapshot }). { $count } archivos no se pudieron leer, así que no están en ella. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
       *[other] Copia de seguridad parcial (instantánea { $snapshot }). { $count } archivos no se pudieron leer, así que no están en ella. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
    }

journal.cli.backup.run.partial.snapshot.count.reason =
    { $quantity ->
        [one] Copia de seguridad parcial (instantánea { $snapshot }). 1 archivo no se pudo leer, así que no está en ella. El primero: { $reason }. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
        [many] Copia de seguridad parcial (instantánea { $snapshot }). { $count } archivos no se pudieron leer, así que no están en ella. El primero: { $reason }. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
       *[other] Copia de seguridad parcial (instantánea { $snapshot }). { $count } archivos no se pudieron leer, así que no están en ella. El primero: { $reason }. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
    }

journal.cli.backup.run.partial.snapshot.unmeasured = Copia de seguridad parcial (instantánea { $snapshot }). algunos archivos no se pudieron leer, así que no están en ella. Ejecuta `{ $command_run }` de nuevo una vez que eso esté corregido.

journal.cli.backup.run.partial.unconfirmed.count =
    { $quantity ->
        [one] Copia de seguridad parcial. 1 archivo no se pudo leer, así que no está en ella, y no se pudo confirmar la instantánea. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
        [many] Copia de seguridad parcial. { $count } archivos no se pudieron leer, así que no están en ella, y no se pudo confirmar la instantánea. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
       *[other] Copia de seguridad parcial. { $count } archivos no se pudieron leer, así que no están en ella, y no se pudo confirmar la instantánea. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
    }

journal.cli.backup.run.partial.unconfirmed.count.reason =
    { $quantity ->
        [one] Copia de seguridad parcial. 1 archivo no se pudo leer, así que no está en ella, y no se pudo confirmar la instantánea. El primero: { $reason }. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
        [many] Copia de seguridad parcial. { $count } archivos no se pudieron leer, así que no están en ella, y no se pudo confirmar la instantánea. El primero: { $reason }. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
       *[other] Copia de seguridad parcial. { $count } archivos no se pudieron leer, así que no están en ella, y no se pudo confirmar la instantánea. El primero: { $reason }. Ejecuta `{ $command }` de nuevo una vez que eso esté corregido.
    }

journal.cli.backup.run.partial.unconfirmed.unmeasured = Copia de seguridad parcial. algunos archivos no se pudieron leer, así que no están en ella, y no se pudo confirmar la instantánea. Ejecuta `{ $command_run }` de nuevo una vez que eso esté corregido.

journal.cli.backup.run.skipped = Copia de seguridad omitida (no activada o no configurada).

journal.cli.backup.teardown.backup.only.held = tu copia de seguridad tiene la única copia de algunos de tus medios, así que no hubo cambios en la copia de seguridad. restaura todo desde la página de copia de seguridad primero y luego vuelve a intentarlo.

journal.cli.backup.teardown.confirm = Se rechaza desmontar la copia de seguridad sin { $yes_flag }. Esto olvida todas las instantáneas.

journal.cli.backup.teardown.not.started = no se pudo empezar a desactivar y eliminar tu copia de seguridad. no hubo cambios en la copia de seguridad.

journal.cli.backup.teardown.partial = desmontaje de la copia de seguridad no terminó. conserva archivos de recuperación y revisa efectos registrados antes de reintentar.

journal.cli.backup.teardown.prelaunch.held = desactivar y eliminar tu copia de seguridad no está disponible aquí. no hubo cambios en la copia de seguridad.

journal.cli.backup.usage.destination = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.backup.usage.missing.destination = { $command }: error: los siguientes argumentos son obligatorios: { $required }

journal.cli.backup.usage.missing.offload = { $command }: error: los siguientes argumentos son obligatorios: { $required }

journal.cli.backup.usage.missing.recovery.key = { $command }: error: los siguientes argumentos son obligatorios: { $required }

journal.cli.backup.usage.missing.root = { $command }: error: los siguientes argumentos son obligatorios: { $required }

journal.cli.backup.usage.offload = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.backup.usage.recovery.key = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.backup.usage.root = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.client.date.short = { $weekday } { $month } { $day }

journal.cli.client.date.short.year = { $weekday } { $month } { $day } '{ $year }

journal.cli.client.erase.backups.conflict = Error: elige { $from_backups } o { $skip_backups }, no ambos.

journal.cli.client.erase.backups.missing = Error: la copia de seguridad no está configurada, así que no hay copias de seguridad de las que quitarlo.

journal.cli.client.erase.backups.next = Se quitará de tus copias de seguridad a continuación.

journal.cli.client.erase.backups.resume = Se quitará de tus copias de seguridad cuando reanudes la copia de seguridad.

journal.cli.client.erase.backups.unavailable = Error: este diario todavía no puede quitar cosas de tus copias de seguridad.

journal.cli.client.erase.backups.unchecked = Error: tu diario no pudo revisar tus copias de seguridad ahora mismo. Vuelve a intentarlo, o pasa { $skip_backups } para borrar sin quitarlo de tus copias de seguridad.

journal.cli.client.error.detail = Error: { $detail }

journal.cli.client.error.malformed = no se pudo leer la respuesta del diario.

journal.cli.client.error.timeout = El diario no respondió a tiempo.

journal.cli.client.error.unreachable = no se pudo conectar con tu diario por { $http }.

journal.cli.client.error.unreadable = El diario devolvió un error ilegible.

journal.cli.client.lock.acquire = no se pudo adquirir el bloqueo sidecar: { $error }

journal.cli.client.lock.open = no se puede abrir el archivo de bloqueo: { $error }

journal.cli.client.missing.argument = Error: falta el argumento { $argument }

journal.cli.client.network.cli.pair.join.hint = vincula este dispositivo con:

journal.cli.client.network.cli.pair.local.network.closed = no se puede iniciar la vinculación. tu diario está cerrado a los dispositivos de tu red. ejecuta `{ $call_open }` para vincular otro dispositivo por tu red, o activa tu red privada para vincular a través del servidor de retransmisión.

journal.cli.client.network.cli.pair.no.lan.address = no se puede iniciar la vinculación. tu diario todavía no está accesible en una dirección de red. activa tu red privada para vincular desde cualquier lugar, o conecta este dispositivo a tu red doméstica.

journal.cli.client.network.clients.empty = No hay clientes autorizados.

journal.cli.client.network.clients.seen = { $fingerprint }  { $label }  visto por última vez { $when }

journal.cli.client.network.device.fingerprint.absent = No hay ningún dispositivo vinculado con la huella { $fingerprint }

journal.cli.client.network.device.label.absent = No hay ningún dispositivo vinculado con la etiqueta { $label }

journal.cli.client.network.device.label.cleared = Etiqueta del dispositivo establecida en: (vacía)

journal.cli.client.network.device.label.set = Etiqueta del dispositivo establecida en: { $label }

journal.cli.client.network.devices.empty = Todavía no hay dispositivos vinculados.

journal.cli.client.network.devices.seen = - { $label } · agregado { $added } · visto por última vez { $seen } [{ $fingerprint }]

journal.cli.client.network.devices.total = Dispositivos vinculados: { $total }

journal.cli.client.network.journal.id = journal id:     { $id }

journal.cli.client.network.label.options = Error: especifica { $label } o { $clear }

journal.cli.client.network.linked.systems.heading = Sistemas vinculados:

journal.cli.client.network.listen.state = { $protocol } state: (consulta { $convey } { $path } para obtener el estado en vivo)

journal.cli.client.network.local.network.open = tu diario está abierto a los dispositivos de tu red. los dispositivos de la misma red pueden vincularse con él directamente.

journal.cli.client.network.local.network.this.pc = tu diario está cerrado a los dispositivos de tu red. la app de { $brand } en esta computadora todavía puede vincularse, y los dispositivos a través del servidor de retransmisión también, cuando tu red privada esté activada.

journal.cli.client.network.local.network.windows.asks = windows puede preguntar si journal puede usar tu red. en una cuenta estándar, un administrador tiene que permitirlo. si windows no preguntó y un dispositivo todavía no puede conectar con tu diario, una decisión anterior en windows puede estar bloqueando journal. un administrador puede permitirlo en { $security }, bajo { $firewall }.

journal.cli.client.network.mark.none = la identidad de tu diario se crea cuando su marca queda fijada durante la configuración inicial.

journal.cli.client.network.mark.unavailable = la marca de tu diario: no disponible ahora

journal.cli.client.network.observer.pause = { $verb } todavía no está disponible.

journal.cli.client.network.operation = operation: { $phase }

journal.cli.client.network.option.float = Error: la opción requiere un valor de punto flotante.

journal.cli.client.network.pair.complete = Solicitud de vinculación completada; el dispositivo debería aparecer en `{ $command }`.

journal.cli.client.network.pair.device = Device: { $device }

journal.cli.client.network.pair.expired = Se agotó el tiempo. El código de vinculación venció.

journal.cli.client.network.pair.fingerprint = { $ca } fingerprint: { $algorithm }{ $fingerprint }

journal.cli.client.network.pair.link = { $token }: { $link }

journal.cli.client.network.pair.paired = Paired: { $label }

journal.cli.client.network.pair.result.date = { $indent }paired_at:{ $indent } { $date }

journal.cli.client.network.pair.result.fingerprint = { $indent }fingerprint: { $fingerprint }

journal.cli.client.network.pair.wait = En espera del sistema vinculado…

journal.cli.client.network.portal.approve = continúa para aprobar → { $url }

journal.cli.client.network.posture.direct = postura: directa

journal.cli.client.network.posture.private = postura: red privada

journal.cli.client.network.private.enrolled = inscrito: sí

journal.cli.client.network.private.link.needs.subscription = tu red privada todavía no está activada. tu consentimiento está guardado. termina de activarla en el portal de servicios y luego actívala aquí de nuevo:

journal.cli.client.network.private.link.setting.up = configurando tu red privada...

journal.cli.client.network.private.link.subscribing = tu red privada todavía no está activada. tu consentimiento está guardado. termina de activarla en el portal de servicios dentro de la hora, y tu diario la toma por su cuenta:

journal.cli.client.network.private.state.enabled = state: activada

journal.cli.client.network.private.state.inconsistent = state: necesita reparación

journal.cli.client.network.private.state.not.enabled = state: no activada

journal.cli.client.network.private.state.other = state: { $state }

journal.cli.client.network.private.state.unknown = state: desconocida

journal.cli.client.network.private.unenrolled = inscrito: no

journal.cli.client.network.reach.direct = Postura de alcance: directa

journal.cli.client.network.reach.private = Postura de alcance: red privada

journal.cli.client.network.relay.url = url del servidor de retransmisión: { $url }

journal.cli.client.network.role.invalid = rol no válido; se esperaba uno de: { $phone }, { $observer }

journal.cli.client.network.status.enrolled = Inscrito:      sí

journal.cli.client.network.status.network.enabled = Red privada: activada

journal.cli.client.network.status.network.inconsistent = Red privada: necesita reparación

journal.cli.client.network.status.network.not.enabled = Red privada: no activada

journal.cli.client.network.status.network.other = Red privada: { $state }

journal.cli.client.network.status.network.unknown = Red privada: desconocida

journal.cli.client.network.status.relay = URL del servidor de retransmisión:     { $url }

journal.cli.client.network.status.relay.absent = URL del servidor de retransmisión:     Ninguna

journal.cli.client.network.status.unenrolled = Inscrito:      no

journal.cli.client.network.time.day =
    { $quantity ->
        [one] hace { $quantity } día
        [many] hace { $quantity } días
       *[other] hace { $quantity } días
        }

journal.cli.client.network.time.hour =
    { $quantity ->
        [one] hace { $quantity } hora
        [many] hace { $quantity } horas
       *[other] hace { $quantity } horas
        }

journal.cli.client.network.time.minute =
    { $quantity ->
        [one] hace { $quantity } minuto
        [many] hace { $quantity } minutos
       *[other] hace { $quantity } minutos
        }

journal.cli.client.network.time.month =
    { $quantity ->
        [one] hace { $quantity } mes
        [many] hace { $quantity } meses
       *[other] hace { $quantity } meses
        }

journal.cli.client.network.time.second =
    { $quantity ->
        [one] hace { $quantity } segundo
        [many] hace { $quantity } segundos
       *[other] hace { $quantity } segundos
        }

journal.cli.client.network.time.week =
    { $quantity ->
        [one] hace { $quantity } semana
        [many] hace { $quantity } semanas
       *[other] hace { $quantity } semanas
        }

journal.cli.client.network.timeout.integer = Error: la opción { $flag } requiere un entero.

journal.cli.client.network.unpaired = Desvinculado.

journal.cli.client.option.required = Error: la opción { $option } es obligatoria.

journal.cli.client.option.unknown = Error: opción desconocida { $option }.

journal.cli.client.option.value = Error: la opción { $option } requiere un argumento.

journal.cli.client.port.invalid = { $variable } debe ser un entero de 1 a 65535.

journal.cli.client.stdin.read = no se pudo leer el { $input } nativo de { $brand }: { $error }

journal.cli.client.transport.timeout = { $method } { $path } excedió el tiempo de espera local de { $service } (connect={ $connect }s, read={ $read }s, total={ $total }s)

journal.cli.client.transport.timeout.open = { $method } { $path } excedió el tiempo de espera local de { $service } (connect={ $connect }s, read=none, total=none)

journal.cli.client.trash.moved = Se movió '{ $name }' a la papelera.

journal.cli.client.trash.moved.day = Se movió '{ $name }' a la papelera. Se borra { $day } a menos que se recupere.

journal.cli.client.trash.moved.failed = Se movió '{ $name }' a la papelera. No se pudo borrar ahora.

journal.cli.client.trash.moved.failed.day = Se movió '{ $name }' a la papelera. No se pudo borrar ahora. Se borra { $day } a menos que se recupere.

journal.cli.importer.arg.archive.conflict.commands = argumentos no reconocidos: comandos en conflicto del archivo de audio

journal.cli.importer.arg.archive.conflict.flags = argumentos no reconocidos: opciones en conflicto del archivo de audio

journal.cli.importer.arg.archive.identity.scope = --archive-identity solo es válido con --audio-archive

journal.cli.importer.arg.archive.identity.sha256 = --archive-identity debe ser una identidad SHA-256

journal.cli.importer.arg.error = solstone journal importer: error: { $detail }

journal.cli.importer.arg.expected.one = argumento { $argument }: se esperaba un argumento

journal.cli.importer.arg.facet.scope = --facet solo es válido al importar un archivo de audio o de texto, o una importación del cuerpo

journal.cli.importer.arg.invalid.int = argumento { $flag }: valor int no válido: '{ $value }'

journal.cli.importer.arg.media.required = los siguientes argumentos son obligatorios: media

journal.cli.importer.arg.offset.limit.scope = argumentos no reconocidos: --offset/--limit solo son válidos con --audio-archive-members

journal.cli.importer.arg.resume.scope = argumentos no reconocidos: --resume solo es válido con --audio-archive

journal.cli.importer.arg.source.zone.scope = argumentos no reconocidos: --source-zone solo es válido con --audio-archive

journal.cli.importer.arg.unrecognized = argumentos no reconocidos: { $arguments }

journal.cli.importer.arg.usage = uso: solstone journal importer [-h] [options] media [timestamp]

journal.cli.importer.audio.archive.admitted.line1 = archivo de audio { $run_id }: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.changed = el archivo de audio cambió después de la vista previa; previsualízalo de nuevo

journal.cli.importer.audio.archive.complete.line1 = archivo de audio { $operation }: { $run_id }

journal.cli.importer.audio.archive.complete.line2 = se guardaron { $parts } en { $tiles }; { $pending } pendientes de procesamiento, { $processed } procesadas, { $unavailable } no disponibles, { $rejected } rechazadas

journal.cli.importer.audio.archive.dry.run.resume = usa --audio-archive-status para revisar una ejecución admitida

journal.cli.importer.audio.archive.failed = el archivo de audio falló: { $error }

journal.cli.importer.audio.archive.folder.needs.zone = una carpeta requiere --source-zone

journal.cli.importer.audio.archive.left.out.header = fuera:

journal.cli.importer.audio.archive.left.out.inline = fuera: { $counts }

journal.cli.importer.audio.archive.members.failed = los miembros del archivo de audio fallaron: { $error }

journal.cli.importer.audio.archive.members.summary = miembros del archivo de audio: run_id={ $run_id } offset={ $offset } count={ $count }

journal.cli.importer.audio.archive.preview.line1 = vista previa del archivo de audio: { $members }, { $source_bytes }, { $days }, { $tiles }

journal.cli.importer.audio.archive.progress.saved = se guardaron { $saved } de { $total } ({ $tiles })

journal.cli.importer.audio.archive.span.line = span: { $span }

journal.cli.importer.audio.archive.span.none = ninguno

journal.cli.importer.audio.archive.span.range = { $first } a { $last }

journal.cli.importer.audio.archive.status.failed = el estado del archivo de audio falló: { $error }

journal.cli.importer.audio.archive.status.summary = estado del archivo de audio: run_id={ $run_id } total_parts={ $total_parts } saved_parts={ $saved_parts } saved_tiles={ $saved_tiles } pending_processing={ $pending } processed_parts={ $processed } unavailable_parts={ $unavailable } rejected_parts={ $rejected } save_complete={ $save_complete }

journal.cli.importer.audio.archive.zone.changed = la zona horaria cambió después de la vista previa; previsualízalo de nuevo

journal.cli.importer.audio.complete = Importación genérica de audio completa: { $outcome }

journal.cli.importer.audio.dropped.notice = no se pudo importar parte de esta grabación: { $ranges } en la grabación. las otras partes están en tu diario, e importar el archivo de nuevo las agregaría una segunda vez.

journal.cli.importer.audio.dropped.range = { $start } a { $end }

journal.cli.importer.audio.failed = la importación de audio falló: { $error }

journal.cli.importer.audio.manifest.failed = la importación de audio se guardó en tu diario, pero no se pudo marcar este archivo como importado, así que importarlo de nuevo lo agregaría una segunda vez: { $error }

journal.cli.importer.audio.processing.failed = el procesamiento de la importación de audio falló: { $segments }

journal.cli.importer.audio.processing.stalled = la importación de audio seguía procesándose cuando el importer dejó de esperar: { $segments }

journal.cli.importer.audio.runtime.failed = el tiempo de ejecución de la importación de audio falló: { $error }

journal.cli.importer.backends.header = Backends sincronizables:

journal.cli.importer.body.apple.already.ran = esta importación ya se ejecutó, así que no se trajo nada. inicia una nueva desde la tarjeta de Apple Health.

journal.cli.importer.body.apple.card.only = --import-id solo es para guardar una exportación de Apple Health desde su tarjeta, así que no se trajo nada.

journal.cli.importer.body.apple.preview.intro = esto parece una exportación de Apple Health. guardarla lee { $head }, y { $tail }.

journal.cli.importer.body.apple.preview.moves =
    { $quantity ->
        [one] decir que sí puede mover hasta 1 registro ya listado bajo otra faceta.
        [many] decir que sí puede mover hasta { $count } registros ya listados bajo otra faceta.
       *[other] decir que sí puede mover hasta { $count } registros ya listados bajo otra faceta.
    }

journal.cli.importer.body.apple.preview.span = va de { $first } a { $last }: { $workouts }, { $nights } reunidas a partir de etapas de sueño, { $mindful }, { $readings }, { $daily } y { $logged }.

journal.cli.importer.body.apple.reads.daily = totales diarios como pasos y distancia

journal.cli.importer.body.apple.reads.logged = entradas registradas como comida y síntomas

journal.cli.importer.body.apple.reads.mindful = tiempo de atención plena

journal.cli.importer.body.apple.reads.readings = frecuencia cardíaca y otras lecturas

journal.cli.importer.body.apple.reads.sleep = sueño

journal.cli.importer.body.apple.reads.workouts = entrenamientos

journal.cli.importer.body.apple.unknown.import = --import-id no nombra ninguna importación, así que no se trajo nada.

journal.cli.importer.body.apple.whole = una importación de Apple Health siempre lee toda la exportación, así que no acepta --date-from, --date-to ni --force. no se guardó nada; ejecútala de nuevo sin ellos.

journal.cli.importer.body.apple.wrong.export = esa importación no contiene esta exportación, así que no se trajo nada.

journal.cli.importer.body.days.hidden =
    { $quantity ->
        [one] 1 día de datos del cuerpo estaba oculto en el archivo comprimido, así que también está oculto en este diario.
        [many] { $count } días de datos del cuerpo estaban ocultos en el archivo comprimido, así que también están ocultos en este diario.
       *[other] { $count } días de datos del cuerpo estaban ocultos en el archivo comprimido, así que también están ocultos en este diario.
    }

journal.cli.importer.body.facet.ask.existing = ¿clasificar tus entrenamientos, el sueño y otros datos del cuerpo bajo la faceta que nombraste { $facet }?

journal.cli.importer.body.facet.ask.make.body = ¿crear una faceta del cuerpo para tus entrenamientos, tu sueño y otros datos del cuerpo?

journal.cli.importer.body.facet.ask.unmute = tu faceta del cuerpo está silenciada. ¿la vuelves a activar para tus entrenamientos, tu sueño y otros datos del cuerpo?

journal.cli.importer.body.facet.moves.body =
    { $quantity ->
        [one] un sí puede mover hasta 1 registro ya listado en la página de actividades, desde { $from }.
        [many] un sí puede mover hasta { $count } registros ya listados en la página de actividades, desde { $from }.
       *[other] un sí puede mover hasta { $count } registros ya listados en la página de actividades, desde { $from }.
    }

journal.cli.importer.body.facet.moves.strava =
    { $quantity ->
        [one] un sí mueve 1 entrenamiento ya clasificado, desde { $from }.
        [many] un sí mueve { $count } entrenamientos ya clasificados, desde { $from }.
       *[other] un sí mueve { $count } entrenamientos ya clasificados, desde { $from }.
    }

journal.cli.importer.body.facet.no.means.body = si dices que no, los entrenamientos nuevos y el sueño no se listan en la página de actividades, y los agentes limitados a algunas facetas no los verán. la búsqueda y la app del cuerpo siguen mostrándolos.

journal.cli.importer.body.facet.no.means.strava = si dices que no, los entrenamientos nuevos no se listan en la página de actividades, no salen personas ni cosas de ellos, y los agentes limitados a algunas facetas no los verán. la búsqueda y la app del cuerpo siguen mostrándolos.

journal.cli.importer.body.facet.prompt.yn = { $question } [y/n]

journal.cli.importer.body.facet.save.failed = no se pudo guardar tu respuesta: { $error }

journal.cli.importer.body.oura.bad.date = { $flag } toma una fecha como 2026-01-31, y { $text } no es una. no se leyó nada.

journal.cli.importer.body.oura.date.to.needs.from = --date-to necesita un --date-from que lo acompañe. no se leyó nada.

journal.cli.importer.body.oura.range.future = ese intervalo pasa de hoy, y Oura todavía no tiene nada ahí. no se leyó nada.

journal.cli.importer.body.oura.window.conflict = usa --window-days o fechas, no ambos. no se leyó nada.

journal.cli.importer.body.oura.window.zero = --window-days necesita al menos un día. no se leyó nada.

journal.cli.importer.body.zones.hours = { $quantity ->
    [one] 1 hora de { $source } puede mostrarse a la hora equivocada: se guardó en una zona horaria distinta de la que este diario guarda para { $source }.
    [many] { $count } horas de { $source } pueden mostrarse a la hora equivocada: se guardaron en una zona horaria distinta de la que este diario guarda para { $source }.
   *[other] { $count } horas de { $source } pueden mostrarse a la hora equivocada: se guardaron en una zona horaria distinta de la que este diario guarda para { $source }.
    }

journal.cli.importer.body.zones.under.hour = menos de una hora de { $source } puede mostrarse a la hora equivocada: se guardó en una zona horaria distinta de la que este diario guarda para { $source }.

journal.cli.importer.connect.available = Backends conectables: oura, plaud

journal.cli.importer.connect.oura.both.required = Oura necesita tanto el ID de cliente como el secreto de cliente, así que no se guardó nada.

journal.cli.importer.connect.oura.done = Oura está conectado, y la conexión está guardada en la configuración de este diario.

journal.cli.importer.connect.oura.scopes = permisos solicitados: { $scopes }

journal.cli.importer.connect.unknown.backend = Backend de conexión desconocido: { $backend }

journal.cli.importer.count.attendee =
    { $quantity ->
        [one] 1 asistente único
        [many] { $count } asistentes únicos
       *[other] { $count } asistentes únicos
    }

journal.cli.importer.count.calendar.event =
    { $quantity ->
        [one] 1 evento de calendario
        [many] { $count } eventos de calendario
       *[other] { $count } eventos de calendario
    }

journal.cli.importer.count.conversation =
    { $quantity ->
        [one] 1 conversación
        [many] { $count } conversaciones
       *[other] { $count } conversaciones
    }

journal.cli.importer.count.daily.total =
    { $quantity ->
        [one] 1 total diario
        [many] { $count } totales diarios
       *[other] { $count } totales diarios
    }

journal.cli.importer.count.day =
    { $quantity ->
        [one] 1 día
        [many] { $count } días
       *[other] { $count } días
    }

journal.cli.importer.count.entity =
    { $quantity ->
        [one] 1 entidad
        [many] { $count } entidades
       *[other] { $count } entidades
    }

journal.cli.importer.count.event =
    { $quantity ->
        [one] 1 evento
        [many] { $count } eventos
       *[other] { $count } eventos
    }

journal.cli.importer.count.facet =
    { $quantity ->
        [one] 1 faceta
        [many] { $count } facetas
       *[other] { $count } facetas
    }

journal.cli.importer.count.logged.entry =
    { $quantity ->
        [one] 1 entrada registrada
        [many] { $count } entradas registradas
       *[other] { $count } entradas registradas
    }

journal.cli.importer.count.member =
    { $quantity ->
        [one] 1 miembro
        [many] { $count } miembros
       *[other] { $count } miembros
    }

journal.cli.importer.count.message =
    { $quantity ->
        [one] 1 mensaje
        [many] { $count } mensajes
       *[other] { $count } mensajes
    }

journal.cli.importer.count.mindful.session =
    { $quantity ->
        [one] 1 sesión de atención plena
        [many] { $count } sesiones de atención plena
       *[other] { $count } sesiones de atención plena
    }

journal.cli.importer.count.night =
    { $quantity ->
        [one] 1 noche
        [many] { $count } noches
       *[other] { $count } noches
    }

journal.cli.importer.count.part =
    { $quantity ->
        [one] 1 parte
        [many] { $count } partes
       *[other] { $count } partes
    }

journal.cli.importer.count.pdf.document =
    { $quantity ->
        [one] 1 documento PDF
        [many] { $count } documentos PDF
       *[other] { $count } documentos PDF
    }

journal.cli.importer.count.reading =
    { $quantity ->
        [one] 1 lectura
        [many] { $count } lecturas
       *[other] { $count } lecturas
    }

journal.cli.importer.count.recording =
    { $quantity ->
        [one] 1 grabación
        [many] { $count } grabaciones
       *[other] { $count } grabaciones
    }

journal.cli.importer.count.segment =
    { $quantity ->
        [one] 1 segmento
        [many] { $count } segmentos
       *[other] { $count } segmentos
    }

journal.cli.importer.count.source.byte =
    { $quantity ->
        [one] 1 byte de fuente
        [many] { $count } bytes de fuente
       *[other] { $count } bytes de fuente
    }

journal.cli.importer.count.tile =
    { $quantity ->
        [one] 1 mosaico
        [many] { $count } mosaicos
       *[other] { $count } mosaicos
    }

journal.cli.importer.count.total.page =
    { $quantity ->
        [one] 1 página total
        [many] { $count } páginas totales
       *[other] { $count } páginas totales
    }

journal.cli.importer.count.warning =
    { $quantity ->
        [one] 1 aviso
        [many] { $count } avisos
       *[other] { $count } avisos
    }

journal.cli.importer.count.workout =
    { $quantity ->
        [one] 1 entrenamiento
        [many] { $count } entrenamientos
       *[other] { $count } entrenamientos
    }

journal.cli.importer.desktop.exe.no.parent = el ejecutable actual no tiene directorio padre

journal.cli.importer.desktop.missing.sibling = falta el ejecutable hermano { $path }

journal.cli.importer.embedded.body.refusal = Las exportaciones de Apple Health y los datos de Oura se traen con un diario en una computadora, así que no se trajo nada.

journal.cli.importer.error.apple.inspect = no se pudo revisar la exportación de Apple Health; reintenta con una exportación válida

journal.cli.importer.error.facet.not.unmuted = "{ $facet }" no es una de tus facetas no silenciadas, así que no se importó nada. tus facetas no silenciadas: { $enabled }

journal.cli.importer.error.facet.scope = --facet solo aplica a un archivo de audio, un archivo de texto sin formato o una importación del cuerpo, así que no se importó nada; importa este sin eso

journal.cli.importer.error.facets.unreadable = no se pudieron leer tus facetas, así que no se importó nada: { $error }

journal.cli.importer.error.no.facet.conflict = argumento --no-facet: no se permite con el argumento --facet

journal.cli.importer.error.no.facet.scope = --no-facet solo aplica a una importación del cuerpo, así que no se importó nada; importa este sin eso

journal.cli.importer.error.preview.failed = no se pudo previsualizar la importación: { $error }

journal.cli.importer.error.stopped = se detuvo la importación

journal.cli.importer.error.timestamp.format = la marca de tiempo debe tener formato YYYYMMDD_HHMMSS

journal.cli.importer.error.timestamp.format.offset = la marca de tiempo debe ser YYYYMMDD_HHMMSS, o una fecha y hora con su desfase, como 2025-11-02T01:30:00-07:00

journal.cli.importer.error.unknown.source = fuente de importer desconocida: { $source }

journal.cli.importer.error.zone.changed.preview = la zona horaria cambió después de la vista previa; previsualiza de nuevo antes de importar

journal.cli.importer.force.day.notice = nota: este archivo data de { $fresh }, y una importación anterior usó { $stored }. esta importación con --force usará { $planned }. las grabaciones existentes no se mueven.

journal.cli.importer.force.pdf.day.notice = nota: el nombre de este archivo data de { $fresh }, y una importación anterior quedó registrada el { $stored }. las fechas propias del PDF pueden elegir un día distinto. elige --timestamp para fijar su fecha. las grabaciones existentes no se mueven.

journal.cli.importer.generic.text.complete = Importación genérica de texto completa: segments={ $segments }

journal.cli.importer.generic.text.untimed.line1 = no se encontraron tiempos de turno en este archivo, así que quedó como un segmento a la hora de inicio de la importación, cada línea tal como está escrita.

journal.cli.importer.generic.text.untimed.line2 = si tus transcripciones marcan los tiempos de una forma que el diario todavía no lee, pídenos que lo agreguemos: { $url }

journal.cli.importer.help.opt.archive.members = { $indent }Listar los miembros de una ejecución de archivo de audio (úsalos con --offset y --limit)

journal.cli.importer.help.opt.archive.status = { $indent }Imprimir el estado compacto de una ejecución de archivo de audio

journal.cli.importer.help.opt.audio.archive.line1 = { $indent }--audio-archive PATH  Importar una carpeta de grabaciones Ogg Opus o un manifiesto JSON

journal.cli.importer.help.opt.audio.archive.line2 = { $indent }Carpeta: los nombres de archivo Limitless y --source-zone son obligatorios.

journal.cli.importer.help.opt.audio.archive.line3 = { $indent }Manifiesto: { $example }

journal.cli.importer.help.opt.audio.archive.line4 = { $indent }O usa local_civil y source_zone para la hora de un miembro.

journal.cli.importer.help.opt.audio.archive.line5 = { $indent }Las rutas son absolutas o relativas al manifiesto.

journal.cli.importer.help.opt.audio.archive.line6 = { $indent }Los tiempos explícitos del manifiesto tienen prioridad sobre los nombres de archivo.

journal.cli.importer.help.opt.audio.archive.line7 = { $indent }Admite Ogg Opus mono/estéreo de un solo flujo; los demás contenedores quedan fuera.

journal.cli.importer.help.opt.audio.archive.line8 = { $indent }Muestra el tamaño antes de guardar; --dry-run no guarda nada.

journal.cli.importer.help.opt.auto = { $indent }--auto [AUTO]         Adoptar la marca de tiempo detectada; AUTO es orientación, no la ruta del archivo

journal.cli.importer.help.opt.backends = { $indent }--backends            Listar los backends sincronizables de importer

journal.cli.importer.help.opt.confirm.body = { $indent }--confirm-body-save   Confirmar que esta ejecución puede guardar salida sensible del importer del cuerpo

journal.cli.importer.help.opt.debug = { $indent }-d, --debug            Activar el registro de depuración

journal.cli.importer.help.opt.deterministic.line1 = { $indent }--deterministic-only  Cuando no se puede leer una fecha del archivo, omítelo (exit 0)

journal.cli.importer.help.opt.deterministic.line2 = { $indent }en lugar de fallar

journal.cli.importer.help.opt.dry.run = { $indent }--dry-run             Mostrar lo que se importaría sin escribir en el diario

journal.cli.importer.help.opt.facet.line1 = { $indent }--facet FACET         Archivar este archivo de audio o de texto, o esta importación del cuerpo, bajo FACET,

journal.cli.importer.help.opt.facet.line2 = { $indent }una de tus facetas que no están silenciadas, en vez de dejar que tu diario elija

journal.cli.importer.help.opt.facet.line3 = { $indent }(para una importación del cuerpo: tu faceta recordada, o ninguna)

journal.cli.importer.help.opt.force = { $indent }--force               Forzar la reimportación; las fuentes del cuerpo crean una importación aparte

journal.cli.importer.help.opt.help = { $indent }-h, --help            mostrar este mensaje de ayuda y salir

journal.cli.importer.help.opt.json.line1 = { $indent }--json                Mostrar los resultados como JSON; los guardados de archivo comprimido emiten objetos de progreso

journal.cli.importer.help.opt.json.line2 = { $indent }y un resultado final, un objeto JSON por línea.

journal.cli.importer.help.opt.limit = { $indent }--limit LIMIT         Máximo de miembros a listar (predeterminado 100, máx. 1000)

journal.cli.importer.help.opt.list.importers = { $indent }--list-importers      Listar los importadores de archivos disponibles

journal.cli.importer.help.opt.no.facet.line1 = { $indent }--no-facet            Archivar esta importación del cuerpo sin faceta, sea cual sea la recordada

journal.cli.importer.help.opt.no.facet.line2 = { $indent }predeterminada es

journal.cli.importer.help.opt.offset = { $indent }--offset OFFSET       Desplazamiento inicial de miembro para el listado de miembros

journal.cli.importer.help.opt.resume = { $indent }--resume IMPORT_ID    Completar las partes no guardadas del plan congelado de un archivo comprimido de audio

journal.cli.importer.help.opt.save = { $indent }--save                Con --sync: descargar e importar archivos nuevos (lo predeterminado es ensayo)

journal.cli.importer.help.opt.source = { $indent }--source SOURCE       Obligatorio para formatos clasificados (ics, exportación de chat, imagen, PDF)

journal.cli.importer.help.opt.source.zone = { $indent }--source-zone IANA    Zona horaria para archivos comprimidos de audio en carpeta u horas civiles (p. ej. America/New_York)

journal.cli.importer.help.opt.timestamp = { $indent }Los .txt/.md genéricos usan esto, o --auto para adoptar una marca de tiempo detectada (--timestamp YYYYMMDD_HHMMSS, o una fecha y hora con su desfase, como 2025-11-02T01:30:00-07:00)

journal.cli.importer.help.opt.verbose = { $indent }-v, --verbose         Activar la salida detallada

journal.cli.importer.help.options.header = opciones:

journal.cli.importer.help.positional.header = argumentos posicionales:

journal.cli.importer.help.positional.media = { $indent }media                 Ruta a un archivo de audio o de texto

journal.cli.importer.help.summary = Importar un archivo multimedia al diario

journal.cli.importer.help.usage = uso: solstone journal importer [-h] [--timestamp TIMESTAMP] [--facet FACET] [--no-facet]

journal.cli.importer.list.desc.apple.health = Previsualizar datos de export.xml de Apple Health sin escribir en el diario

journal.cli.importer.list.desc.chatgpt = Importar conversaciones de una exportación de ChatGPT

journal.cli.importer.list.desc.claude = Importar conversaciones de una exportación de chat de Claude

journal.cli.importer.list.desc.document = Importar documentos PDF con extracción de texto y ráster mediante un proceso

journal.cli.importer.list.desc.evernote = Notas de una exportación de Evernote

journal.cli.importer.list.desc.gemini = Importar conversaciones de una exportación de Gemini o Bard de Google Takeout

journal.cli.importer.list.desc.ics = Importar eventos de archivos de calendario ICS o de un ZIP de exportación de Google Calendar

journal.cli.importer.list.desc.image = Importar una sola imagen y describir su contenido con visión

journal.cli.importer.list.desc.journal.archive = Fusionar un archivo comprimido de diario exportado en el diario actual

journal.cli.importer.list.desc.oura = Previsualizar documentos JSON de la API v2 de Oura (datos de prueba sintéticos; la ruta de guardado es una fase posterior y condicionada)

journal.cli.importer.list.desc.strava = Entrenamientos de una descarga de Strava (solo se lee su activities.csv)

journal.cli.importer.list.header = Importadores de archivos:

journal.cli.importer.notes.folder.retired = importar una carpeta de notas ya no está; una bóveda de Obsidian entra a tu diario cuando la activas en la app de solstone en la computadora donde vive la bóveda

journal.cli.importer.oura.app.needed = este diario todavía no está conectado a una app que hayas registrado con Oura. registra una, luego ejecuta `solstone journal importer --connect oura` en una terminal y pega su ID de cliente y su secreto de cliente. cómo: { $how }

journal.cli.importer.oura.app.prompt.client.id = id de cliente:

journal.cli.importer.oura.app.prompt.client.secret = secreto de cliente:

journal.cli.importer.oura.app.prompt.intro = este diario todavía no está conectado a una app que hayas registrado con Oura. para conectar una:

journal.cli.importer.oura.app.prompt.note = se guardan en la configuración de este diario y se usan solo para conectar con Oura. no se guarda nada hasta que Oura diga que sí. cómo: { $how }

journal.cli.importer.oura.app.prompt.step1 = { $indent }1. inicia sesión en https://cloud.ouraring.com/oauth/applications y crea una app

journal.cli.importer.oura.app.prompt.step2 = { $indent }2. agrega el URI de redirección http://localhost:8765/callback

journal.cli.importer.oura.app.prompt.step3 = { $indent }3. pega abajo el ID de cliente y el secreto de cliente de la app

journal.cli.importer.oura.connect.failed.browser = no se pudo abrir un navegador aquí. ejecuta esto en la computadora del diario, no por una sesión remota.

journal.cli.importer.oura.connect.failed.callback = El inicio de sesión de Oura vuelve a esta computadora en el puerto 8765, y algo más lo está usando. cierra eso y vuelve a ejecutarlo.

journal.cli.importer.oura.connect.failed.declined = Oura no se conectó, así que no se guardó nada.

journal.cli.importer.oura.connect.failed.exchange = Oura no aceptó la conexión, así que no se guardó nada. revisa el secreto de cliente en la página de tu app en Oura y vuelve a ejecutarlo.

journal.cli.importer.oura.connect.failed.timeout = El inicio de sesión de Oura no terminó en cinco minutos, así que no se guardó nada. vuelve a ejecutarlo para reintentar.

journal.cli.importer.plaud.begin.invalid.redirect = uri de redirección de callback de Plaud no válido

journal.cli.importer.plaud.begin.loopback.bind = no se pudo enlazar el receptor local de callback

journal.cli.importer.plaud.begin.random.failed = no se pudo generar el identificador de inicio de sesión

journal.cli.importer.plaud.connect.detail.busy = ya hay otro inicio de sesión u otra sincronización en curso

journal.cli.importer.plaud.connect.detail.callback.invalid = la dirección de inicio de sesión no era válida

journal.cli.importer.plaud.connect.detail.cancelled = inicio de sesión cancelado

journal.cli.importer.plaud.connect.detail.denied = no se aprobó el inicio de sesión

journal.cli.importer.plaud.connect.detail.exchange.failed = no se pudo terminar de iniciar sesión

journal.cli.importer.plaud.connect.detail.expired = se agotó el tiempo del intento de inicio de sesión

journal.cli.importer.plaud.connect.detail.storage = no se pudo guardar el acceso en este diario

journal.cli.importer.plaud.connect.done = { $provider } está conectado, y tu acceso está guardado en este diario.

journal.cli.importer.plaud.connect.done.account = { $provider } está conectado como { $account }, y tu acceso está guardado en este diario.

journal.cli.importer.plaud.connect.failed = no se pudo conectar con { $provider }: { $detail }

journal.cli.importer.plaud.connect.next = ejecuta { $command } para traer grabaciones ahora, o activa la sincronización cada hora en la configuración.

journal.cli.importer.plaud.connect.open = abre este enlace para iniciar sesión con { $provider }:

journal.cli.importer.plaud.connect.retry.url = esa no era la dirección de inicio de sesión de { $provider }. pega la dirección completa de la barra del navegador, o termina en el navegador.

journal.cli.importer.plaud.connect.waiting = en espera de que { $provider } termine el inicio de sesión. si tu navegador está en otro dispositivo, pega la dirección final de la barra del navegador y presiona enter:

journal.cli.importer.plaud.failure.audio.not.ready = el enlace de descarga del audio todavía no está listo; se volverá a intentar en la siguiente sincronización

journal.cli.importer.plaud.failure.catalogue = falló el catálogo

journal.cli.importer.plaud.failure.download = falló la descarga

journal.cli.importer.plaud.failure.manifest = falló el emparejamiento de la importación

journal.cli.importer.plaud.failure.pipeline = no se pudo importar

journal.cli.importer.plaud.failure.temporary.url = no se pudo obtener la URL de descarga

journal.cli.importer.plaud.failure.token.refused = el acceso está vencido o fue revocado; vuelve a iniciar sesión en la configuración.

journal.cli.importer.plaud.preview.complete = Vista previa de sincronización de Plaud completa: files={ $files }

journal.cli.importer.plaud.sync.busy = tu diario ya está revisando { $provider }. vuelve a intentarlo cuando termine.

journal.cli.importer.plaud.sync.missing.credential = La credencial de Plaud no está configurada

journal.cli.importer.plaud.sync.operation = Plaud { $detail }

journal.cli.importer.plaud.sync.state.failed = Falló el estado de sincronización de Plaud: { $message }

journal.cli.importer.preview.chatgpt = { $messages } de una exportación de ChatGPT

journal.cli.importer.preview.claude = { $messages } de una exportación de chat de Claude

journal.cli.importer.preview.document.counts = { $documents }, { $pages }

journal.cli.importer.preview.document.none = No se encontraron documentos PDF

journal.cli.importer.preview.document.unreadable = { $documents }, { $pages }; { $unreadable } ilegibles ({ $failures })

journal.cli.importer.preview.gemini = { $messages } de una exportación de Gemini

journal.cli.importer.preview.ics.counts = { $events }, { $attendees }

journal.cli.importer.preview.ics.no.events = No se encontraron eventos en los datos ICS

journal.cli.importer.preview.ics.none = No se encontraron datos ICS

journal.cli.importer.preview.image.one = 1 imagen ({ $format }, { $width }×{ $height })

journal.cli.importer.registry.archive.error.and.record.failed = falló la importación de { $source }: { $error }; no se pudo registrar el intento: { $record_error }

journal.cli.importer.registry.archive.missing.facts = falló la importación de { $source }: faltan los hechos del intento para { $attempt }

journal.cli.importer.registry.archive.missing.provenance = falló la importación de { $source }: falta la procedencia del intento { $attempt }

journal.cli.importer.registry.archive.record.attempt.failed = falló la importación de { $source }: no se pudo registrar el intento: { $error }

journal.cli.importer.registry.archive.record.completed.failed = falló la importación de { $source }: no se pudo registrar el intento completado: { $error }

journal.cli.importer.registry.archive.record.merge.failed = falló la importación de { $source }: no se pudieron registrar los resultados de la fusión: { $error }

journal.cli.importer.registry.attempt.superseded = falló la importación de { $source }: el intento { $import_id }:{ $generation } fue sustituido

journal.cli.importer.registry.evernote.attachments.failed = Falló la importación de Evernote. { $error }

journal.cli.importer.registry.found.nothing = falló la importación de { $source }: no se encontró nada para importar

journal.cli.importer.registry.gemini.folder.unreadable = Falló la importación de Gemini. no se pudo leer la carpeta de exportación guardada.

journal.cli.importer.registry.originals.choice.unreadable = no se pudo leer la opción de conservar los originales. vuelve a abrir la importación para volver a intentarlo.

journal.cli.importer.registry.originals.cleanup.failed = se guardó en tu diario. no se pudo quitar la copia extra.

journal.cli.importer.registry.originals.keep.failed = no se pudo conservar una copia del original. vuelve a intentarlo.

journal.cli.importer.registry.preview.failed = falló la vista previa de la importación: { $error }

journal.cli.importer.registry.strava.discard.failed = no se pudo quitar la descarga preparada de Strava: { $error }

journal.cli.importer.registry.strava.filing.failed = falló la importación de { $source }: no se pudieron archivar los entrenamientos: { $error }

journal.cli.importer.registry.strava.release.failed = no se pudieron liberar los entrenamientos colocados a la hora incorrecta

journal.cli.importer.registry.summary.image = se importó 1 imagen

journal.cli.importer.registry.summary.pdf = se importó { $documents }

journal.cli.importer.registry.unconfirmed = la importación de { $source } se guardó en tu diario, pero no se pudo confirmar que las entradas estén listas para buscar; vuelve a ejecutarla

journal.cli.importer.size.listed.moves =
    { $quantity ->
        [one] 1 entrenamiento que ya está en la página de actividades se movería a la faceta de esta importación desde { $from }
        [many] { $count } entrenamientos que ya están en la página de activities se moverían a la faceta de esta importación desde { $from }
       *[other] { $count } entrenamientos que ya están en la página de activities se moverían a la faceta de esta importación desde { $from }
    }

journal.cli.importer.size.recordings.across = { $recordings } a lo largo de { $days }

journal.cli.importer.size.skipped = { $count } omitidos

journal.cli.importer.skipped.already = Importación omitida: este archivo ya se importó; usa --force para importarlo de nuevo

journal.cli.importer.skipped.confirm.date = Importación omitida: la fecha del archivo necesita confirmación; vuelve a ejecutarlo con --timestamp o --auto

journal.cli.importer.skipped.no.date = Importación omitida: no se pudo leer ninguna fecha del archivo; nombra una con --timestamp YYYYMMDD_HHMMSS

journal.cli.importer.source.archive.already.present = la importación de { $source } no fusionó nada: el contenido del archivo comprimido ya está presente

journal.cli.importer.source.archive.complete = importación de { $source } completa: segments_copied={ $segments_copied } imports_copied={ $imports_copied } entities_created={ $entities_created } entities_merged={ $entities_merged } facets_created={ $facets_created } facets_merged={ $facets_merged }

journal.cli.importer.source.archive.incomplete = importación de { $source } incompleta: { $detail }

journal.cli.importer.source.complete = importación de { $source } completa: entries_written={ $entries } files_created={ $files } errors={ $errors } summary={ $summary }

journal.cli.importer.source.failed = falló la importación de { $source }: { $detail }

journal.cli.importer.source.preview = vista previa de { $source }: date_range={ $start }..{ $end } items={ $items } entities={ $entities } summary={ $summary }

journal.cli.importer.summary.archive = exportación del diario: { $days }, { $entities }, { $facets } ({ $warnings })

journal.cli.importer.summary.conversations = se importaron { $messages } de { $conversations } a lo largo de { $days }

journal.cli.importer.summary.document = se importaron { $documents } a lo largo de { $days } en { $segments }

journal.cli.importer.summary.ics.base = se importaron { $events } a lo largo de { $days }

journal.cli.importer.summary.ics.over.limit = ; { $count } sin listar por el día en que ocurren, más allá de las { $limit } fechas que lista una importación

journal.cli.importer.summary.ics.reason.invalid = { $count } ilegibles

journal.cli.importer.summary.ics.reason.no.change = { $count } sin hora de cambio

journal.cli.importer.summary.ics.reason.unknown.zone = { $count } en una zona horaria desconocida

journal.cli.importer.summary.ics.skipped = ; { $skipped } sin listar por el día en que ocurren ({ $reasons })

journal.cli.importer.summary.ics.truncated.series =
    { $quantity ->
        [one] ; 1 serie periódica listada solo para sus últimas { $limit } fechas
        [many] ; { $count } series periódicas listadas solo para sus últimas { $limit } fechas
       *[other] ; { $count } series periódicas listadas solo para sus últimas { $limit } fechas
    }

journal.cli.importer.summary.ics.unexpandable.series =
    { $quantity ->
        [one] ; 1 serie periódica listada solo para su primera fecha
        [many] ; { $count } series periódicas listadas solo para su primera fecha
       *[other] ; { $count } series periódicas listadas solo para su primera fecha
    }

journal.cli.importer.summary.image.none = No se encontró ninguna imagen legible

journal.cli.importer.sync.available = Backends disponibles: { $backends }

journal.cli.importer.sync.complete = sincronización de { $backend } completa: saved={ $saved } errors={ $errors }

journal.cli.importer.sync.unknown.backend = Backend de sincronización desconocido: { $backend }

journal.cli.importer.text.failed = falló la importación de texto: { $error }

journal.cli.importer.text.unconfirmed = la importación de texto se guardó en tu diario, pero no se pudo confirmar que las entradas estén listas para buscar; vuelve a ejecutarla

journal.cli.importer.timestamp.confirm = marca de tiempo detectada { $timestamp }; vuelve a ejecutarlo con --timestamp { $timestamp } o --auto

journal.cli.oauth.callback.failure = el inicio de sesión no terminó. vuelve a tu diario y vuelve a intentarlo

journal.cli.oauth.callback.success = iniciaste sesión. puedes cerrar esta pestaña y volver a tu diario

journal.cli.oauth.oura.callback.allowed = lo permitiste en Oura. puedes cerrar esta pestaña; la terminal dice cuándo está conectado.

journal.cli.oauth.oura.callback.declined = Oura no se conectó. puedes cerrar esta pestaña.

journal.cli.oauth.oura.callback.rejected = Autorización de Oura rechazada.

journal.cli.setup.clean.changed.during.service = la instalación cambió mientras se quitaba su servicio

journal.cli.setup.clean.child.exited = la desinstalación del servicio terminó con { $code }

journal.cli.setup.clean.child.exited.detail = la desinstalación del servicio terminó con { $code }: { $detail }

journal.cli.setup.clean.child.timed.out = la desinstalación del servicio terminó con { $code } (se agotó el tiempo)

journal.cli.setup.clean.child.timed.out.detail = la desinstalación del servicio terminó con { $code } (se agotó el tiempo): { $detail }

journal.cli.setup.clean.config.invalid.toml = la configuración no es TOML válido

journal.cli.setup.clean.config.io = no se pudo leer la configuración: { $error }

journal.cli.setup.clean.config.missing.journal = la configuración no tiene la clave journal

journal.cli.setup.clean.config.not.absolute = la ruta journal de la configuración no es absoluta

journal.cli.setup.clean.config.not.string = el valor journal de la configuración no es una cadena

journal.cli.setup.clean.confirm.config = { $indent }[{ $marker }] config: { $path }

journal.cli.setup.clean.confirm.header = solstone journal setup --clean-uninstall quitará estos artefactos de tiempo de ejecución:

journal.cli.setup.clean.confirm.keep.header = no se quitará:

journal.cli.setup.clean.confirm.keep.journal = { $indent }- directorio del diario: { $path }

journal.cli.setup.clean.confirm.keep.macos.app = { $indent }- /Applications/solstone.app

journal.cli.setup.clean.confirm.keep.macos.permissions = { $indent }- permisos de micrófono o de grabación de pantalla de macOS

journal.cli.setup.clean.confirm.keep.macos.support = { $indent }- ~/Library/Application Support/solstone/

journal.cli.setup.clean.confirm.keep.python = { $indent }- una instalación residual de journal con pip, uv o pipx

journal.cli.setup.clean.confirm.manifest = { $indent }[{ $marker }] manifiesto del diario: { $path }

journal.cli.setup.clean.confirm.marker.absent = ausente

journal.cli.setup.clean.confirm.marker.present = presente

journal.cli.setup.clean.confirm.marker.retain = retener

journal.cli.setup.clean.confirm.service = { $indent }[{ $marker }] service: { $path }

journal.cli.setup.clean.confirm.service.unavailable = { $indent }la ubicación del servicio no está disponible: { $error }

journal.cli.setup.clean.confirm.wrapper = { $indent }[{ $marker }] wrapper: { $path }

journal.cli.setup.clean.journal.line = journal: { $path }

journal.cli.setup.clean.no.binding = se rechazó la desinstalación limpia: no hay una identidad de instalación adoptada

journal.cli.setup.clean.no.owned.path = sin ruta propia

journal.cli.setup.clean.outcome.already.complete = la desinstalación limpia ya está completa

journal.cli.setup.clean.outcome.cancelled = se canceló

journal.cli.setup.clean.outcome.complete = desinstalación limpia completa: { $removed } quitados, { $already_absent } ya ausentes, { $preserved } conservados, { $skipped } omitidos, { $failed } fallidos

journal.cli.setup.clean.outcome.no.eligible = no hay archivos elegibles de la configuración inicial para quitar

journal.cli.setup.clean.outcome.not.tty = no es una tty; vuelve a ejecutarlo con --yes para continuar de forma no interactiva (se canceló)

journal.cli.setup.clean.outcome.service.unavailable = la ubicación del servicio no está disponible: { $error }

journal.cli.setup.clean.outcome.stopped.failed = la desinstalación limpia se detuvo después de un paso fallido

journal.cli.setup.clean.preflight.census.error = el censo de identidad de la instalación es ilegible: { $error }

journal.cli.setup.clean.preflight.census.unreadable = el censo de identidad de la instalación es ilegible

journal.cli.setup.clean.preflight.config.mismatch = el journal de la configuración no coincide con la identidad de la instalación

journal.cli.setup.clean.preflight.failed = falló la comprobación previa de la desinstalación limpia: { $error }

journal.cli.setup.clean.preflight.overlap = el almacenamiento de identidad se superpone a un diario protegido

journal.cli.setup.clean.preflight.prepared = la identidad de la instalación está preparada

journal.cli.setup.clean.readmit.failed = no se pudo admitir la instalación de nuevo después de quitar su servicio: { $error }

journal.cli.setup.clean.reason.manifest.preserved = el manifiesto del diario se conserva

journal.cli.setup.clean.reason.no.owned.path.known = no se conoce una ruta propia

journal.cli.setup.clean.reason.no.owned.path.platform = no hay una ruta propia en esta plataforma

journal.cli.setup.clean.reason.preserved.mark = preserved: { $mark }

journal.cli.setup.clean.reason.preserved.non.dir = destino conservado que no es un directorio

journal.cli.setup.clean.reason.preserved.symlink = enlace simbólico conservado

journal.cli.setup.clean.reason.service.guard.mismatch = la guarda del servicio o del wrapper no coincide con esta instalación, no se quita

journal.cli.setup.clean.reason.skill.foreign = el destino del enlace simbólico es ajeno

journal.cli.setup.clean.reason.skill.preserved = se conserva tu propio contenido de skills

journal.cli.setup.clean.reason.skipped.after.failure = el paso no se ejecutó después de un fallo anterior

journal.cli.setup.clean.reason.wrapper.cross.repo = el alias apunta a { $target }, no se quita

journal.cli.setup.clean.reason.wrapper.dangling = el alias está colgante (falta el destino { $target }), no se quita

journal.cli.setup.clean.reason.wrapper.foreign = el alias no es un enlace simbólico administrado, no se quita

journal.cli.setup.clean.reason.wrapper.guard.mismatch = la guarda del wrapper no coincide con esta instalación, no se quita

journal.cli.setup.clean.reason.wrapper.unexpected = estado de alias inesperado: { $state }

journal.cli.setup.clean.reason.wrapper.unknown = desconocido

journal.cli.setup.clean.reason.wrapper.worktree = se rechaza actuar desde un worktree de git

journal.cli.setup.clean.refusal.incompatible = --clean-uninstall no se puede combinar con { $flags }

journal.cli.setup.clean.refusal.jsonl = JSONL no se admite como salida para --clean-uninstall en esta versión.

journal.cli.setup.clean.refused = se rechazó la desinstalación limpia: { $reason }

journal.cli.setup.clean.step.running = [paso { $step }/{ $total }] ejecutando la desinstalación de { $name }...

journal.cli.setup.error.home.dir = la configuración inicial no pudo determinar tu directorio personal

journal.cli.setup.error.refused.before.mutation = solstone journal setup: se rechazó antes de la modificación: { $error }

journal.cli.setup.error.sign.in.windows.only = solstone journal setup: --initial-start-at-sign-in solo está disponible en Windows

journal.cli.setup.host.journal.binding.changed = el enlace del diario cambió

journal.cli.setup.host.packaged.speech.unavailable = prueba de voz empaquetada no disponible

journal.cli.setup.identity.adopted.no.marker = el registro adoptado no tiene marcador de adopción

journal.cli.setup.identity.changed.retry = la instalación cambió mientras se cambiaba la carpeta de su diario; vuelve a intentarlo

journal.cli.setup.identity.executable.unavailable = el ejecutable actual no está disponible

journal.cli.setup.identity.finish.recovery.first = termina la configuración inicial o la recuperación antes de cambiar la carpeta del diario

journal.cli.setup.identity.finish.setup.first = termina la configuración inicial del diario antes de cambiar su carpeta

journal.cli.setup.identity.foreign.artifacts = los artefactos existentes de wrapper o de servicio están ligados a una instalación distinta

journal.cli.setup.identity.generation.decimal = la generación no es un decimal canónico

journal.cli.setup.identity.generation.nonzero = la generación debe ser distinta de cero

journal.cli.setup.identity.generation.overflow = desbordamiento de generación

journal.cli.setup.identity.generation.range = la generación está fuera de rango

journal.cli.setup.identity.guard.duplicate = guarda de identidad duplicada { $field }

journal.cli.setup.identity.guard.incomplete = la guarda del artefacto no es una identidad coincidente completa

journal.cli.setup.identity.guard.invalid = guarda de identidad no válida { $field }

journal.cli.setup.identity.guard.mismatch = la guarda del artefacto no coincide con la identidad

journal.cli.setup.identity.guard.missing = los campos de la guarda de identidad están incompletos

journal.cli.setup.identity.guard.root.mismatch = la guarda del artefacto no coincide con la raíz admitida

journal.cli.setup.identity.guarded.no.record = los artefactos existentes de wrapper o de servicio tienen una guarda, pero no existe un registro de instalación coincidente en el almacenamiento de identidad

journal.cli.setup.identity.home.unset = HOME no está definido

journal.cli.setup.identity.io.create.home = crear el directorio personal de la configuración inicial: { $error }

journal.cli.setup.identity.legacy.manifest.invalid = la evidencia del manifiesto legado está malformada o es ilegible

journal.cli.setup.identity.legacy.needs.manifest = los artefactos legados necesitan un manifiesto schema-v1 válido sin proveedor

journal.cli.setup.identity.namespace.mismatch = el registro del espacio de nombres no coincide con su espacio de nombres raíz

journal.cli.setup.identity.no.bootstrap.evidence = los artefactos existentes no tienen una evidencia válida de arranque

journal.cli.setup.identity.not.found = la identidad de la instalación no existe

journal.cli.setup.identity.owner.root.changed = la raíz de admisión del propietario cambió durante el bloqueo

journal.cli.setup.identity.path.outside = PATH se resuelve fuera de la instalación V2

journal.cli.setup.identity.prepared.uninstall = la identidad preparada no se puede desinstalar

journal.cli.setup.identity.provider.app.dir = la base del proveedor de la instalación no tiene directorio de la aplicación

journal.cli.setup.identity.provider.parent = la base del proveedor de la instalación no tiene padre

journal.cli.setup.identity.record.stage.malformed = etapa de registro malformada

journal.cli.setup.identity.record.stage.stale = la etapa de registro desactualizada no coincide con su nombre

journal.cli.setup.identity.root.unavailable = la raíz de la instalación no está disponible

journal.cli.setup.identity.tombstoned.no.marker = el registro con lápida no tiene marcador de adopción

journal.cli.setup.identity.uncertain.artifacts = el enlace del artefacto está malformado o es ambiguo

journal.cli.setup.identity.windows.path.odd = La ruta de Windows tiene un número impar de bytes

journal.cli.setup.identity.windows.platform.mismatch = la plataforma del propietario no coincide con esta compilación de Windows

journal.cli.setup.identity.wrapper.journal.unsafe = la ruta efectiva del diario no se puede representar de forma segura en los wrappers administrados

journal.cli.setup.narrate.artifacts.header = artefactos:

journal.cli.setup.narrate.artifacts.none = { $indent }ninguno

journal.cli.setup.narrate.close.finish.unix = termina la configuración inicial: solstone journal open /init

journal.cli.setup.narrate.close.finish.url = termina la configuración inicial: { $url }/init

journal.cli.setup.narrate.close.open.unix = abre tu diario: solstone journal open

journal.cli.setup.narrate.close.open.url = abre tu diario: { $url }

journal.cli.setup.narrate.close.skip.service.unix = una vez que tu diario esté en ejecución, ábrelo con: solstone journal open

journal.cli.setup.narrate.close.skip.service.url = una vez que tu diario esté en ejecución, ábrelo en { $url }

journal.cli.setup.narrate.complete.counts = { $skipped } de { $total } pasos ya hechos; se ejecutaron { $ran }

journal.cli.setup.narrate.complete.installed = solstone está instalado.

journal.cli.setup.narrate.complete.set.up = solstone está configurado.

journal.cli.setup.narrate.model.recovery.direct = la instalación del modelo no terminó. ejecuta { $journal } install-models --variant { $variant }, luego { $journal } setup.

journal.cli.setup.narrate.model.recovery.installer = la instalación del modelo no terminó. ejecuta { $journal } install-models --variant { $variant }.

journal.cli.setup.narrate.prior.clean.force = solstone journal setup tuvo su última ejecución correcta el { $completed_at }; volviendo a ejecutar todos los pasos (--force).

journal.cli.setup.narrate.prior.clean.verify = solstone journal setup tuvo su última ejecución correcta el { $completed_at }; verificando el estado actual.

journal.cli.setup.narrate.prior.force.hint = Usa --force para volver a ejecutar todos los pasos incondicionalmente.

journal.cli.setup.narrate.prior.incomplete.header = solstone journal setup dejó estos pasos incompletos en la última ejecución del { $started_at }:

journal.cli.setup.narrate.prior.incomplete.hint = Volver a ejecutar verificará el estado y repetirá los pasos incompletos.

journal.cli.setup.narrate.prior.incomplete.item = { $indent }- { $name } (falló)

journal.cli.setup.narrate.sol.keeps.journal = solstone en esta Mac ya conserva este diario, así que la configuración inicial no instaló un lanzador en segundo plano.{ $line_break }Ejecuta solstone journal doctor si algo se ve mal.

journal.cli.setup.narrate.step.failed = solstone journal setup: { $name } falló: { $message }

journal.cli.setup.narrate.step.running = [paso { $index }/{ $total }] ejecutando { $name }...

journal.cli.setup.narrate.step.running.command = [paso { $index }/{ $total }] ejecutando { $name }: { $command }

journal.cli.setup.narrate.step.skipped = [paso { $index }/{ $total }] omitido { $name }: { $reason }

journal.cli.setup.parser.cwd = no se pudo determinar el directorio actual: { $error }

journal.cli.setup.parser.path.empty = { $journal_flag } no debe estar vacío

journal.cli.setup.parser.path.encoding = { $journal_flag } no se pudo resolver: la ruta no es { $encoding }

journal.cli.setup.parser.path.resolve = { $journal_flag } no se pudo resolver: { $error }

journal.cli.setup.parser.port = { $port_flag } debe estar en { $range } (se obtuvo { $value })

journal.cli.setup.parser.sign.in.encoding = argumento { $sign_in_flag }: se esperaba { $on_value } o { $off_value }

journal.cli.setup.parser.sign.in.invalid = argumento { $sign_in_flag }: opción no válida: { $value } (elige entre { $choices })

journal.cli.setup.parser.sign.in.repeated = argumento { $sign_in_flag }: no debe repetirse

journal.cli.setup.parser.timeout = argumento { $timeout_flag }: valor entero no válido

journal.cli.setup.parser.unknown = argumentos no reconocidos

journal.cli.setup.parser.variant = argumento { $variant_flag }: opción no válida: { $value } (elige entre { $choices })

journal.cli.setup.path.added = path: se agregó ~/.local/bin al PATH del shell

journal.cli.setup.path.added.restart = path: se agregó ~/.local/bin al PATH del shell — reinicia tu shell o ejecuta 'exec $SHELL -l' para que lo cargue

journal.cli.setup.path.already = path: ~/.local/bin ya está en PATH

journal.cli.setup.path.manual = path: no se pudo agregar automáticamente ~/.local/bin a PATH — agrega esta línea al rc de tu shell manualmente: export PATH="$HOME/.local/bin:$PATH"

journal.cli.setup.path.windows = Los paquetes de Windows exponen solstone directamente; no se instaló un wrapper de PATH POSIX.

journal.cli.setup.plan.brain = establecería el carril del proveedor local si esta computadora puede ejecutar el modelo local, y lo arrancaría si nada lo bloquea

journal.cli.setup.plan.doctor = ejecutaría: { $binary } journal doctor --readiness

journal.cli.setup.plan.heading.dry.run = ensayo de la configuración inicial:

journal.cli.setup.plan.heading.explain = plan de la configuración inicial:

journal.cli.setup.plan.identity.admission = admisión de identidad: se ejecutaría antes de los pasos de la configuración inicial que hacen cambios

journal.cli.setup.plan.install.models = ejecutaría: { $binary } journal install-models --variant { $variant }

journal.cli.setup.plan.journal = { $indent }journal: { $path } ({ $source })

journal.cli.setup.plan.mode = { $indent }mode: { $mode }

journal.cli.setup.plan.model.download.default = { $indent }descarga del modelo: sin límite de tiempo de la configuración inicial (por defecto)

journal.cli.setup.plan.port = { $indent }port: { $value } ({ $source })

journal.cli.setup.plan.service = instalaría e iniciaría el servicio del diario

journal.cli.setup.plan.skills.journal = ejecutaría: { $binary } skills install --project { $journal } --agent all

journal.cli.setup.plan.skills.user = ejecutaría: { $binary } skills install --agent all

journal.cli.setup.plan.source.checkout.false = { $indent }checkout del código fuente: False

journal.cli.setup.plan.source.checkout.true = { $indent }checkout del código fuente: True

journal.cli.setup.plan.step.header = [paso { $index }/{ $total }] { $name }

journal.cli.setup.plan.step.timeout = { $indent }step_timeout_seconds: { $value } ({ $source })

journal.cli.setup.plan.variant = { $indent }variant: { $value } ({ $source })

journal.cli.setup.plan.wrapper.unix = aprovisionaría los wrappers administrados de solstone y journal en el mismo proceso

journal.cli.setup.plan.wrapper.windows = omitiría aprovisionar el wrapper POSIX; el paquete de Windows expone los comandos directamente

journal.cli.setup.plan.write.config = escribiría: { $path }

journal.cli.setup.prompt.existing.journal = { $path } ya contiene datos del diario; ¿continuar? [y/N]:

journal.cli.setup.prompt.proceed = ¿continuar? [y/N]:

journal.cli.setup.service.restart.exited = el reinicio del servicio terminó con { $code }

journal.cli.setup.warning.launcher.ownership = aviso: no se pudo inspeccionar la propiedad del lanzador de solstone: { $error }

journal.cli.setup.warning.manifest = aviso: no se pudo escribir el manifiesto de la configuración inicial: { $error }

journal.cli.setup.warning.package.receipt = solstone journal setup: aviso: no se escribió el recibo del paquete: { $error }

journal.cli.setup.wrapper.retired.note = se quitaron las herramientas de línea de comandos de la versión anterior; sus copias de seguridad están en ~/.local/share/solstone/setup-backups

journal.cli.setup.wrapper.skip.path = se dejaron sin cambios los archivos de inicio de sesión del shell porque se seleccionó --skip-path

journal.cli.think.activity.blocked = actividad bloqueada

journal.cli.think.activity.body.data = esta actividad viene de un entrenamiento de { $provider }, que son datos del cuerpo; no se ejecuta nada más sobre ella.

journal.cli.think.activity.day.missing = falta el archivo del día de la actividad: { $path }

journal.cli.think.activity.fingerprint.failed = no se puede calcular la huella de las entradas de la actividad

journal.cli.think.activity.malformed = la declaración de la faceta está mal formada. corrígela y luego reintenta con { $reactivate_flag }.

journal.cli.think.activity.missing = falta la faceta de destino. después de restaurar su declaración, reintenta con { $reactivate_flag }; si no, reclasifica el segmento de origen.

journal.cli.think.activity.muted.blocked = la faceta de destino está silenciada. quita el silencio y luego reintenta la actividad bloqueada con { $reactivate_flag }.

journal.cli.think.activity.muted.retry = la faceta de destino está silenciada. quita el silencio para reanudar el procesamiento de la actividad sin terminar.

journal.cli.think.activity.owner.field.held = la salida de la actividad está retenida por el propietario

journal.cli.think.activity.pending = reintento de actividad pendiente

journal.cli.think.activity.read.blocked = no se pudo leer la declaración de la faceta. después de restaurar el acceso, reintenta la actividad bloqueada con { $reactivate_flag }.

journal.cli.think.activity.read.retry = no se pudo leer la declaración de la faceta. la actividad sin terminar se reintentará automáticamente.

journal.cli.think.activity.record.missing = no existe un registro de actividad para esta clasificación. después de corregir la declaración, reprocesa el segmento de origen.

journal.cli.think.activity.repair.required = requiere reparación

journal.cli.think.activity.replaced = se reemplazó la faceta de destino. reclasifica el segmento de origen; un resultado anterior no se puede aplicar al reemplazo.

journal.cli.think.activity.retry.identity = no coincide la identidad del trabajo de actividad

journal.cli.think.activity.retry.kind = no es trabajo de actividad

journal.cli.think.activity.retry.muted = la faceta de la actividad está silenciada

journal.cli.think.activity.routing = corrige la declaración y luego reprocesa este segmento de origen.

journal.cli.think.activity.row.missing = falta la fila de la actividad: { $path }

journal.cli.think.activity.settled = { $facet }/{ $activity } resuelta: esta actividad viene de un entrenamiento de { $provider }, que son datos del cuerpo; no se ejecuta nada más sobre ella.

journal.cli.think.activity.state = no se guardó el estado de la actividad: { $error }

journal.cli.think.activity.state.segment = { $segment }: no se guardó el estado de la actividad: { $error }

journal.cli.think.activity.work.malformed = trabajo de actividad mal formado: { $path }

journal.cli.think.activity.work.pending = trabajo de actividad pendiente: { $failures }

journal.cli.think.activity.work.running = la entrada anterior de la actividad sigue en ejecución

journal.cli.think.activity.work.unreadable = no se puede leer el trabajo de actividad { $path }: { $error }

journal.cli.think.activity.work.version = versión de trabajo de actividad no admitida

journal.cli.think.argument.integer = argumento { $option }: valor entero no válido

journal.cli.think.argument.unknown = argumentos no reconocidos: { $argument }

journal.cli.think.argument.value = argumento { $option }: se esperaba un argumento

journal.cli.think.cancelled = se canceló el razonamiento

journal.cli.think.daily.blockers.more = { $names }, +{ $count } más

journal.cli.think.daily.incomplete =
    { $units ->
        [one] { $blockers ->
            [one] { $phase } ({ $units } unidad diaria no terminal; { $blockers } bloqueo de segmento permanece{ $details })
            [many] { $phase } ({ $units } unidad diaria no terminal; { $blockers } bloqueo(s) de segmento permanecen{ $details })
           *[other] { $phase } ({ $units } unidad diaria no terminal; { $blockers } bloqueo(s) de segmento permanecen{ $details })
        }
        [many] { $blockers ->
            [one] { $phase } ({ $units } unidad(es) diaria(s) no terminal; { $blockers } bloqueo de segmento permanece{ $details })
            [many] { $phase } ({ $units } unidad(es) diaria(s) no terminal; { $blockers } bloqueo(s) de segmento permanecen{ $details })
           *[other] { $phase } ({ $units } unidad(es) diaria(s) no terminal; { $blockers } bloqueo(s) de segmento permanecen{ $details })
        }
       *[other] { $blockers ->
            [one] { $phase } ({ $units } unidad(es) diaria(s) no terminal; { $blockers } bloqueo de segmento permanece{ $details })
            [many] { $phase } ({ $units } unidad(es) diaria(s) no terminal; { $blockers } bloqueo(s) de segmento permanecen{ $details })
           *[other] { $phase } ({ $units } unidad(es) diaria(s) no terminal; { $blockers } bloqueo(s) de segmento permanecen{ $details })
        }
    }

journal.cli.think.daily.incomplete.record = { $phase } ({ $units } unidad(es) diaria(s) no terminal; { $blockers } bloqueo(s) de segmento permanecen{ $details })

journal.cli.think.daily.inputs.changed = la entrada en bruto cambió durante el procesamiento del día completo

journal.cli.think.daily.location.packet = la reutilización de la ubicación diaria no tiene un paquete almacenado

journal.cli.think.daily.location.read = no se pudo leer la ubicación diaria

journal.cli.think.daily.snapshot.changed = la evidencia diaria cambió antes de la preparación

journal.cli.think.daily.snapshot.changed.after = la evidencia diaria o el contrato cambió durante la preparación

journal.cli.think.daily.snapshot.freeze = preparación de la entrada diaria: { $error }

journal.cli.think.daily.snapshot.parent = la raíz del talento no tiene padre

journal.cli.think.daily.snapshot.removed = se quitó el talento diario durante la preparación

journal.cli.think.error = { $command }: { $message }

journal.cli.think.error.with.log = { $primary }; { $log }

journal.cli.think.health.day.invalid = día no válido { $day }; se esperaba { $date_format }

journal.cli.think.health.directory = no se puede leer el directorio { $path }: { $detail }

journal.cli.think.health.marker = error del marcador de estado: { $detail }

journal.cli.think.health.marker.atomic = no se pudo publicar el marcador de estado: { $error }

journal.cli.think.health.marker.fingerprint = no se pudo leer la huella del marcador de estado: { $detail }

journal.cli.think.health.marker.io = falló la E/S del marcador de estado en { $path }: { $error }

journal.cli.think.health.marker.lock = no se pudo obtener el bloqueo del marcador de estado: { $error }

journal.cli.think.health.marker.malformed = el marcador de estado está mal formado: { $path }

journal.cli.think.health.marker.stream = el marcador de estado del stream está mal formado

journal.cli.think.health.metadata = no se pueden leer los metadatos del archivo { $path }: { $detail }

journal.cli.think.health.progress.unreadable = el progreso del segmento contiene registros ilegibles

journal.cli.think.health.read = error de E/S del diario: { $detail }

journal.cli.think.health.source = error de fuente del registro de estado: { $detail }

journal.cli.think.help = uso: { $syntax }

journal.cli.think.host.sense.missing = el ejecutor de Sense alojado no está disponible

journal.cli.think.intent.gone = la solicitud destructiva ya no está en espera

journal.cli.think.log.activity.join = { $command }: { $day }/{ $segment } tardío se unió a su actividad abierta

journal.cli.think.log.activity.no.advance = { $command }: { $day }/{ $segment } no avanza el estado en vivo de la actividad

journal.cli.think.log.activity.replay = { $command }: { $day }/{ $segment } tardío se vuelve a ejecutar con el día de su stream

journal.cli.think.log.activity.turn = avanzando el estado de la actividad de { $stream } sin su turno: { $error }

journal.cli.think.log.budget = el resultado de think supera el presupuesto de registros del registro operativo

journal.cli.think.log.cadence.read = No se pudo cargar el estado de cadencia: { $error }

journal.cli.think.log.cadence.set.aside = No se pudo cargar el estado de cadencia: artefacto corrupto apartado

journal.cli.think.log.daily.search = no se pudo inspeccionar el trabajo de búsqueda antes del razonamiento diario: { $error }

journal.cli.think.log.day.format = el día debe ser { $date_format }

journal.cli.think.log.day.local = día local no válido

journal.cli.think.log.display.metadata = no se pudieron guardar los metadatos de visualización de { $path }: { $error }

journal.cli.think.log.failed = falló el registro de ejecución de think { $path }: { $detail }

journal.cli.think.log.failure.lock = bloqueo de fallos envenenado del registro de ejecución de think

journal.cli.think.log.held.release = no se pudo soltar { $name } en retención para { $segment }: { $error }

journal.cli.think.log.lock = bloqueo envenenado del registro de ejecución de think

journal.cli.think.log.name.answers = respuestas de nombre: { $answers } cambiaron, { $judged } actividades juzgadas de nuevo, { $changed } cambiaron, { $waiting } en espera

journal.cli.think.log.provider =
    { $left ->
        [one] el proveedor de razonamiento no está disponible; { $left } segmento espera la próxima ejecución
        [many] el proveedor de razonamiento no está disponible; { $left } segmentos esperan la próxima ejecución
       *[other] el proveedor de razonamiento no está disponible; { $left } segmentos esperan la próxima ejecución
    }

journal.cli.think.log.provider.record = el proveedor de razonamiento no está disponible; { $left } segmentos esperan la próxima ejecución

journal.cli.think.log.reconciliation = falló la conciliación del reintento: { $error }

journal.cli.think.log.segment.turn = razonamiento de { $day }/{ $segment } sin su turno: { $error }

journal.cli.think.markdown.entities = Entidades de Sense

journal.cli.think.markdown.entity = { $kind } — { $name } (role={ $role }, source={ $source }) — { $context }

journal.cli.think.markdown.object.both =
    { $frames ->
        [one] { $photos ->
            [one] { $class }: en { $frames } fotograma y { $photos } foto
            [many] { $class }: en { $frames } fotograma y { $photos } fotos
           *[other] { $class }: en { $frames } fotograma y { $photos } fotos
        }
        [many] { $photos ->
            [one] { $class }: en { $frames } fotogramas y { $photos } foto
            [many] { $class }: en { $frames } fotogramas y { $photos } fotos
           *[other] { $class }: en { $frames } fotogramas y { $photos } fotos
        }
       *[other] { $photos ->
            [one] { $class }: en { $frames } fotogramas y { $photos } foto
            [many] { $class }: en { $frames } fotogramas y { $photos } fotos
           *[other] { $class }: en { $frames } fotogramas y { $photos } fotos
        }
    }

journal.cli.think.markdown.object.both.maximum =
    { $frames ->
        [one] { $photos ->
            [one] { $class }: en { $frames } fotograma y { $photos } foto, hasta { $most } a la vez
            [many] { $class }: en { $frames } fotograma y { $photos } fotos, hasta { $most } a la vez
           *[other] { $class }: en { $frames } fotograma y { $photos } fotos, hasta { $most } a la vez
        }
        [many] { $photos ->
            [one] { $class }: en { $frames } fotogramas y { $photos } foto, hasta { $most } a la vez
            [many] { $class }: en { $frames } fotogramas y { $photos } fotos, hasta { $most } a la vez
           *[other] { $class }: en { $frames } fotogramas y { $photos } fotos, hasta { $most } a la vez
        }
       *[other] { $photos ->
            [one] { $class }: en { $frames } fotogramas y { $photos } foto, hasta { $most } a la vez
            [many] { $class }: en { $frames } fotogramas y { $photos } fotos, hasta { $most } a la vez
           *[other] { $class }: en { $frames } fotogramas y { $photos } fotos, hasta { $most } a la vez
        }
    }

journal.cli.think.markdown.object.frames =
    { $frames ->
        [one] { $class }: en { $frames } fotograma
        [many] { $class }: en { $frames } fotogramas
       *[other] { $class }: en { $frames } fotogramas
    }

journal.cli.think.markdown.object.frames.maximum =
    { $frames ->
        [one] { $class }: en { $frames } fotograma, hasta { $most } a la vez
        [many] { $class }: en { $frames } fotogramas, hasta { $most } a la vez
       *[other] { $class }: en { $frames } fotogramas, hasta { $most } a la vez
    }

journal.cli.think.markdown.object.photos =
    { $photos ->
        [one] { $class }: en { $photos } foto
        [many] { $class }: en { $photos } fotos
       *[other] { $class }: en { $photos } fotos
    }

journal.cli.think.markdown.object.photos.maximum =
    { $photos ->
        [one] { $class }: en { $photos } foto, hasta { $most } a la vez
        [many] { $class }: en { $photos } fotos, hasta { $most } a la vez
       *[other] { $class }: en { $photos } fotos, hasta { $most } a la vez
    }

journal.cli.think.markdown.objects = Objetos

journal.cli.think.markdown.partial.counts = Quedó afuera: { $details }

journal.cli.think.markdown.partial.input = Quedó afuera: parte de la entrada no cupo. Este resultado se escribió con una parte de la entrada.

journal.cli.think.markdown.partial.story = Esta historia, y todo lo guardado con ella, se escribió a partir de una parte de la actividad, no de toda.

journal.cli.think.markdown.quotes = Citas

journal.cli.think.markdown.sense.entities = ### Entidades

journal.cli.think.markdown.sense.facets = ### Facetas

journal.cli.think.markdown.sense.record = ## Sense: { $kind }

journal.cli.think.markdown.sense.speakers = **Hablantes:** { $names }

journal.cli.think.markdown.speaker = Hablante { $number }

journal.cli.think.markdown.terminal.clock = { $clock } · terminal

journal.cli.think.markdown.terminal.notice = texto mostrado en tu terminal. esto dice lo que estaba visible, no lo que leíste, escribiste o hiciste. enmascarar contraseñas y otros secretos es un mejor esfuerzo; tus reglas de redacción no se aplican.

journal.cli.think.markdown.topics = Temas

journal.cli.think.outcome.completed = { $command }: { $count } completados

journal.cli.think.outcome.failed = { $command }: { $count } fallidos

journal.cli.think.phase.daily.prerequisites = el trabajo diario está en espera de los prerrequisitos de su segmento

journal.cli.think.phase.exit = salida { $code }

journal.cli.think.phase.segment.prerequisites = cambiaron los prerrequisitos del segmento

journal.cli.think.plan.activity.count = segments: { $segments }

journal.cli.think.plan.activity.empty = Ningún agente coincide con el tipo de actividad '{ $kind }'

journal.cli.think.plan.activity.heading =
    { $operation ->
        [refresh] Día { $day } { $activity_flag } { $activity } { $facet_flag } { $facet } (actualizar)
       *[ordinary] Día { $day } { $activity_flag } { $activity } { $facet_flag } { $facet }
    }

journal.cli.think.plan.activity.missing = No se encontró la actividad: { $activity } en la faceta '{ $facet }' el { $day }

journal.cli.think.plan.activity.type = type:     { $kind }

journal.cli.think.plan.cadence = Día { $day } — agentes de cadencia

journal.cli.think.plan.cadence.fire =
    { $segments ->
        [one] { $activities ->
            [one] disparo  { $name } — ventana: { $segments } segmento, { $activities } actividad
            [many] disparo  { $name } — ventana: { $segments } segmento, { $activities } actividades
           *[other] disparo  { $name } — ventana: { $segments } segmento, { $activities } actividades
        }
        [many] { $activities ->
            [one] disparo  { $name } — ventana: { $segments } segmentos, { $activities } actividad
            [many] disparo  { $name } — ventana: { $segments } segmentos, { $activities } actividades
           *[other] disparo  { $name } — ventana: { $segments } segmentos, { $activities } actividades
        }
       *[other] { $activities ->
            [one] disparo  { $name } — ventana: { $segments } segmentos, { $activities } actividad
            [many] disparo  { $name } — ventana: { $segments } segmentos, { $activities } actividades
           *[other] disparo  { $name } — ventana: { $segments } segmentos, { $activities } actividades
        }
    }

journal.cli.think.plan.cadence.fire.record = disparo  { $name } — ventana: { $segments } segmento(s), { $activities } actividad(es)

journal.cli.think.plan.cadence.idle = no-op { $name } — no hay trabajo nuevo desde la última ejecución de cadencia

journal.cli.think.plan.cadence.skip = skip  { $name } — el intervalo no ha transcurrido ({ $seconds }s < { $minutes }m)

journal.cli.think.plan.day = Día { $day }

journal.cli.think.plan.day.refresh = Día { $day } (actualizar)

journal.cli.think.plan.day.segment = Día { $day } segmento { $segment }

journal.cli.think.plan.day.segment.refresh = Día { $day } segmento { $segment } (actualizar)

journal.cli.think.plan.empty.cadence = No hay indicaciones para el horario: { $cadence }

journal.cli.think.plan.empty.weekly = No hay indicaciones para el horario: { $weekly }

journal.cli.think.plan.flush = Día { $day } { $flush_flag } segmento { $segment }

journal.cli.think.plan.flush.empty = Ningún agente elegible para flush

journal.cli.think.plan.flush.talent =
    { $kind ->
        [generate] gen  { $name }
       *[agent] agent  { $name }
    }

journal.cli.think.plan.idle = segmentos inactivos: escribir solo stubs (a menos que { $refresh_flag })

journal.cli.think.plan.orchestrator = orquestador de Sense (lineal):

journal.cli.think.plan.post.indexer = fase posterior: { $command }

journal.cli.think.plan.post.stats = fase posterior: { $command }

journal.cli.think.plan.pre = Fase previa:  { $command } { $day } { $jobs_flag } { $jobs }

journal.cli.think.plan.priority = Prioridad { $priority }:

journal.cli.think.plan.reprocess =
    { $count ->
        [one] Día { $day } — reprocesar { $count } segmento
        [many] Día { $day } — reprocesar { $count } segmentos
       *[other] Día { $day } — reprocesar { $count } segmentos
    }

journal.cli.think.plan.reprocess.record = Día { $day } — reprocesar { $count } segmentos

journal.cli.think.plan.schedule.empty = No hay indicaciones para el horario: { $schedule }

journal.cli.think.plan.segment.row = [{ $index }/{ $total }] { $segment } ({ $start }-{ $end }) stream={ $stream }

journal.cli.think.plan.skip = omitir { $name } — sin actividad: { $facets }

journal.cli.think.plan.state = máquina de estados de la actividad: actualizaciones por segmento

journal.cli.think.plan.step.entities =
    { $presence ->
        [existing] { $step }. { $name } ({ $kind }/{ $format }) (ya existe) — siempre para los no inactivos
        [new] { $step }. { $name } ({ $kind }/{ $format }) (nuevo) — siempre para los no inactivos
       *[plain] { $step }. { $name } ({ $kind }/{ $format }) — siempre para los no inactivos
    }

journal.cli.think.plan.step.possible =
    { $presence ->
        [existing] { $step }. { $name } ({ $kind }/{ $format }) (ya existe) — si es una transcripción de voz (nunca bloquea el segmento)
        [new] { $step }. { $name } ({ $kind }/{ $format }) (nuevo) — si es una transcripción de voz (nunca bloquea el segmento)
       *[plain] { $step }. { $name } ({ $kind }/{ $format }) — si es una transcripción de voz (nunca bloquea el segmento)
    }

journal.cli.think.plan.step.screen =
    { $presence ->
        [existing] { $step }. { $name } ({ $kind }/{ $format }) (ya existe) — si { $recommend_screen_record }
        [new] { $step }. { $name } ({ $kind }/{ $format }) (nuevo) — si { $recommend_screen_record }
       *[plain] { $step }. { $name } ({ $kind }/{ $format }) — si { $recommend_screen_record }
    }

journal.cli.think.plan.step.sense =
    { $presence ->
        [existing] { $step }. { $name } ({ $kind }/{ $format }) (ya existe) — obligatorio
        [new] { $step }. { $name } ({ $kind }/{ $format }) (nuevo) — obligatorio
       *[plain] { $step }. { $name } ({ $kind }/{ $format }) — obligatorio
    }

journal.cli.think.plan.step.speaker.attribution =
    { $presence ->
        [existing] { $step }. { $name } ({ $kind }/{ $format }) (ya existe) — si hay incrustaciones de audio (la capa del modelo solo si { $recommend_speaker_attribution })
        [new] { $step }. { $name } ({ $kind }/{ $format }) (nuevo) — si hay incrustaciones de audio (la capa del modelo solo si { $recommend_speaker_attribution })
       *[plain] { $step }. { $name } ({ $kind }/{ $format }) — si hay incrustaciones de audio (la capa del modelo solo si { $recommend_speaker_attribution })
    }

journal.cli.think.plan.table.facet =
    { $row ->
        [generator_existing] gen  { $name }/{ $facet } (ya existe)
        [generator_new] gen  { $name }/{ $facet } (nuevo)
        [generator_plain] gen  { $name }/{ $facet }
        [agent_existing] agent  { $name }/{ $facet } (ya existe)
        [agent_new] agent  { $name }/{ $facet } (nuevo)
       *[agent_plain] agent  { $name }/{ $facet }
    }

journal.cli.think.plan.table.talent =
    { $row ->
        [generator_existing] gen  { $name } (ya existe)
        [generator_new] gen  { $name } (nuevo)
        [generator_plain] gen  { $name }
        [agent_existing] agent  { $name } (ya existe)
        [agent_new] agent  { $name } (nuevo)
       *[agent_plain] agent  { $name }
    }

journal.cli.think.plan.total =
    { $total ->
        [one] Total: { $total } agente
        [many] Total: { $total } agentes
       *[other] Total: { $total } agentes
    }

journal.cli.think.plan.total.record = Total: { $total } agentes

journal.cli.think.plan.weekly = Día { $day } — agentes semanales

journal.cli.think.preflight.disk = Tu diario se quedó sin espacio en su disco ({ $detail }). Reparación: libera espacio en ese disco y luego vuelve a iniciar tu diario.

journal.cli.think.preflight.held = { $reason }: { $role_key }{ $role } { $pid_key }{ $pid } { $time_key }{ $started } { $generation_key }{ $generation } { $age_key }{ $age }; usa la ruta supervisada o detén ese proceso de forma limpia

journal.cli.think.preflight.held.unavailable = { $reason }: { $owner }; usa la ruta supervisada o detén el proceso actual de forma limpia

journal.cli.think.priority.environment = prioridad de trabajo no válida: { $error }

journal.cli.think.priority.syntax = prioridad de trabajo no válida; se esperaba una banda 1–6 y un tiempo de la fuente

journal.cli.think.repair.segment.missing = falta el segmento de reparación

journal.cli.think.reprocess.record = no se pudo registrar el resultado del reproceso: { $error }

journal.cli.think.retry.activity.changed = cambió el permiso del reintento de la actividad

journal.cli.think.retry.activity.released = la actividad ya no está apartada

journal.cli.think.retry.daily.changed = cambió el permiso del reintento diario

journal.cli.think.retry.daily.missing = falta el permiso diario

journal.cli.think.retry.daily.released = la unidad diaria ya no está retenida

journal.cli.think.retry.daily.terminal.missing = falta el registro terminal diario

journal.cli.think.retry.daily.unresolved = el reintento diario sigue sin resolver

journal.cli.think.retry.daily.unused = el escritor diario no consumió el permiso de reintento

journal.cli.think.retry.day.mismatch = el día del reintento no coincide

journal.cli.think.retry.evidence.missing = falta la evidencia del reintento

journal.cli.think.retry.permission.changed = cambió el permiso de reintento

journal.cli.think.retry.raw.coordinate.invalid = coordenada de reintento nativo no válida

journal.cli.think.retry.raw.coordinate.utf8 = coordenada de reintento no UTF8

journal.cli.think.retry.raw.fingerprint.invalid = la huella de la entrada del fallo nativo está malformada

journal.cli.think.retry.raw.handler.missing = el fallo nativo no tiene manejador

journal.cli.think.retry.raw.header.incomplete = el encabezado de procesamiento nativo está incompleto

journal.cli.think.retry.raw.header.kind = el encabezado de procesamiento nativo no es un objeto

journal.cli.think.retry.raw.kind = no es análisis nativo

journal.cli.think.retry.raw.parent.missing = la salida nativa no tiene padre

journal.cli.think.retry.raw.permission.unused = el escritor nativo no consumió el permiso de reintento

journal.cli.think.retry.raw.receipt.changed = cambió el recibo terminal nativo

journal.cli.think.retry.raw.receipt.missing = falta el recibo terminal nativo

journal.cli.think.retry.raw.receipt.unpublished = el escritor nativo no ha publicado el recibo terminal de este reintento

journal.cli.think.retry.raw.receipt.unreadable = el recibo terminal nativo es ilegible

journal.cli.think.retry.raw.record.malformed = el registro de procesamiento nativo está malformado

journal.cli.think.retry.raw.released = la salida nativa ya no está retenida

journal.cli.think.retry.raw.schema.unknown = el esquema de procesamiento nativo no se reconoce

journal.cli.think.retry.raw.source.invalid = el fallo nativo no tiene un nombre de archivo de origen seguro

journal.cli.think.retry.raw.source.mismatch = la fuente del fallo nativo no coincide con su salida

journal.cli.think.retry.segment.changed = cambió el permiso del reintento del segmento

journal.cli.think.retry.talent.disabled = el talento retenido ya no está activado

journal.cli.think.retry.talent.dispatch = no se pudo despachar el talento retenido

journal.cli.think.retry.talent.segment.missing = el segmento ya no existe

journal.cli.think.retry.talent.skipped = no se pudo reintentar el talento retenido

journal.cli.think.routing.log.malformed = no se puede leer la recuperación de enrutamiento desde registros de estado malformados

journal.cli.think.routing.source.invalid = la fuente de enrutamiento retenida no es válida

journal.cli.think.segment.ambiguous =
    { $count ->
        [one] segmento ambiguo { $day }/{ $segment }: { $count } flujo comparte este nombre
        [many] segmento ambiguo { $day }/{ $segment }: { $count } flujos comparten este nombre
       *[other] segmento ambiguo { $day }/{ $segment }: { $count } flujos comparten este nombre
    }

journal.cli.think.sense.complete = Procesamiento por lotes completo

journal.cli.think.sense.config.concurrency = sense: { $handler }.{ $setting } no válido; se usa 1 de forma predeterminada

journal.cli.think.sense.config.read = sense: configuración del diario no disponible: { $error }

journal.cli.think.sense.config.runtime = sense: { $handler }.{ $setting } no válido; se usa { $seconds }s de forma predeterminada

journal.cli.think.sense.conflict = { $sense_batch_flag } es incompatible con { $options }

journal.cli.think.sense.day = { $sense_batch_flag } requiere { $day_flag }

journal.cli.think.sense.day.missing = No se encontró el directorio del día: { $path }

journal.cli.think.sense.deferred =
    { $quantity ->
        [one] Procesamiento por lotes diferido: { $deferred } de { $ran } manejador en espera; los archivos sin terminar se reintentarán
        [many] Procesamiento por lotes diferido: { $deferred } de { $ran } manejadores en espera; los archivos sin terminar se reintentarán
       *[other] Procesamiento por lotes diferido: { $deferred } de { $ran } manejadores en espera; los archivos sin terminar se reintentarán
    }

journal.cli.think.sense.dry.empty = No se encontraron archivos sin procesar

journal.cli.think.sense.dry.would.process =
    { $quantity ->
        [one] se procesaría { $count } archivo ({ $breakdown }):
        [many] se procesarían { $count } archivos ({ $breakdown }):
       *[other] se procesarían { $count } archivos ({ $breakdown }):
    }

journal.cli.think.sense.error.delete = no se pudo eliminar { $path }: { $error }

journal.cli.think.sense.error.deleted.dirty = el reproceso quitó { $path }, pero no se pudo marcar de forma durable el día { $day } como sucio: { $detail }

journal.cli.think.sense.error.failed = { $failed } fallidos de { $ran } ejecutados

journal.cli.think.sense.error.held = la concesión del lote de sense para el día { $day } la tiene otro proceso

journal.cli.think.sense.error.join = El proceso de Sense no se detuvo de forma limpia

journal.cli.think.sense.error.lease = error de la concesión del lote de sense: { $error }

journal.cli.think.sense.error.runtime = runtime de callosum del lote no disponible

journal.cli.think.sense.error.scan = no se pudo examinar { $path }: { $error }

journal.cli.think.sense.error.timeout = se agotó el tiempo del lote de sense después de { $duration }

journal.cli.think.sense.files =
    { $quantity ->
        [one] Se encontró { $count } archivo sin procesar para procesar ({ $extensions })
        [many] Se encontraron { $count } archivos sin procesar para procesar ({ $extensions })
       *[other] Se encontraron { $count } archivos sin procesar para procesar ({ $extensions })
    }

journal.cli.think.sense.interrupted.write = no se pudo registrar el evento { $event } en { $path }: { $error }

journal.cli.think.sense.lease.skip = La concesión del lote de Sense para el día { $day } está retenida; se omite el recorrido del lote

journal.cli.think.sense.nothing = No se encontraron archivos sin procesar en { $path }

journal.cli.think.sense.priority.encoding = { $variable } contiene UTF-8 no válido

journal.cli.think.sense.processing.day =
    { $quantity ->
        [one] Procesando archivos del día { $day } con { $jobs } trabajo concurrente
        [many] Procesando archivos del día { $day } con { $jobs } trabajos concurrentes
       *[other] Procesando archivos del día { $day } con { $jobs } trabajos concurrentes
    }

journal.cli.think.sense.processing.segment =
    { $quantity ->
        [one] Procesando archivos del día { $day } (segmento: { $segment }) con { $jobs } trabajo concurrente
        [many] Procesando archivos del día { $day } (segmento: { $segment }) con { $jobs } trabajos concurrentes
       *[other] Procesando archivos del día { $day } (segmento: { $segment }) con { $jobs } trabajos concurrentes
    }

journal.cli.think.sense.refused = se denegó el lote de sense: { $reason }

journal.cli.think.sense.reprocess.deleted.count =
    { $quantity ->
        [one] se eliminó { $count } archivo de salida
        [many] se eliminaron { $count } archivos de salida
       *[other] se eliminaron { $count } archivos de salida
    }

journal.cli.think.sense.reprocess.deleted.file = deleted: { $path }

journal.cli.think.sense.reprocess.dry.empty = no hay archivos para eliminar

journal.cli.think.sense.reprocess.dry.would.delete =
    { $quantity ->
        [one] se eliminaría { $count } archivo de salida:
        [many] se eliminarían { $count } archivos de salida:
       *[other] se eliminarían { $count } archivos de salida:
    }

journal.cli.think.sense.reprocess.kept.before.voice.only =
    { $quantity ->
        [one] se conservó { $count } transcripción hecha antes de que "dejar fuera otras voces" estuviera activado: desactívalo para reprocesar esa transcripción
        [many] se conservaron { $count } transcripciones hechas antes de que "dejar fuera otras voces" estuviera activado: desactívalo para reprocesarlas
       *[other] se conservaron { $count } transcripciones hechas antes de que "dejar fuera otras voces" estuviera activado: desactívalo para reprocesarlas
    }

journal.cli.think.sense.reprocess.kept.voice.only =
    { $quantity ->
        [one] se conservó { $count } archivo de salida hecho con "dejar fuera otras voces" activado: no se reprocesa
        [many] se conservaron { $count } archivos de salida hechos con "dejar fuera otras voces" activado: no se reprocesan
       *[other] se conservaron { $count } archivos de salida hechos con "dejar fuera otras voces" activado: no se reprocesan
    }

journal.cli.think.sense.retry.changed = cambiaron las entradas del reintento nativo

journal.cli.think.sense.retry.handler = manejador de reintento nativo desconocido

journal.cli.think.sense.retry.kind = el reintento no es análisis nativo

journal.cli.think.sense.retry.mismatch = el manejador del fallo nativo no coincide con su fuente

journal.cli.think.sense.worker.authority = la autoridad de inicio de { $handler } no estaba gestionada

journal.cli.think.sense.worker.deferred = diferido (salida { $code })

journal.cli.think.sense.worker.detail = sense: day={ $day } stream={ $stream } segment={ $segment } file={ $file } error={ $error }

journal.cli.think.sense.worker.exit = { $handler } salida { $code }

journal.cli.think.sense.worker.failed = falló (salida { $code })

journal.cli.think.sense.worker.launch = falló el arranque de { $handler }

journal.cli.think.sense.worker.launch.failed = sense: falló el inicio de { $handler }: { $error }

journal.cli.think.sense.worker.log.tail = sense: diagnóstico del manejador log={ $log } tail={ $tail }

journal.cli.think.sense.worker.log.unreadable = sense: diagnóstico del manejador log={ $log } ilegible: { $error }

journal.cli.think.sense.worker.output.incomplete = { $handler } terminó con 0 con salida sin terminar (salida de análisis incompleta): { $path }

journal.cli.think.sense.worker.output.nonregular = { $handler } terminó con 0 sin una salida regular: { $path }

journal.cli.think.sense.worker.output.unfinished = { $handler } terminó con 0 con salida sin terminar ({ $reason }): { $path }

journal.cli.think.sense.worker.output.unreadable = { $handler } terminó con 0 sin salida legible { $path }: { $error }

journal.cli.think.sense.worker.pool = grupo de { $handler } no disponible

journal.cli.think.sense.worker.program.executable = { $handler } falló current_exe del despachador: { $error }

journal.cli.think.sense.worker.program.missing = { $handler } falta el despachador: { $path }

journal.cli.think.sense.worker.program.nonexecutable = { $handler } el despachador no es ejecutable: { $path }

journal.cli.think.sense.worker.program.nonregular = { $handler } el despachador no es un archivo: { $path }

journal.cli.think.sense.worker.status = sense: handler={ $handler } file={ $file } outcome={ $outcome } log={ $log }

journal.cli.think.sense.worker.survivors = sense: árbol de procesos no recolectado handler={ $handler } reason={ $reason } survivors={ $count }

journal.cli.think.sense.worker.termination = sense: falló la terminación gestionada handler={ $handler }: { $error }

journal.cli.think.sense.worker.timeout = { $handler } watchdog_timeout después de { $seconds }s

journal.cli.think.sense.worker.watchdog = falló la espera del watchdog o del proceso

journal.cli.think.settle.save = no se guardó el estado de asentamiento de la actividad: { $error }

journal.cli.think.updated.capability = capacidad de puesta al día no disponible en esta plataforma

journal.cli.think.updated.conflict = { $updated_flag } es incompatible con { $options }

journal.cli.think.updated.io = falló la E/S de puesta al día en { $path }: { $error }

journal.cli.think.updated.marker = error del marcador de puesta al día: { $error }

journal.cli.think.updated.path = error de la ruta del diario en la puesta al día: { $error }

journal.cli.think.updated.state = el estado de puesta al día está malformado: { $detail }

journal.cli.think.usage.error = uso: { $syntax }{ $line_break }{ $command }: error: { $message }

journal.cli.think.validation.activity.incompatible = { $activity_flag } es incompatible con { $segment_flag }, { $segments_flag } y { $flush_flag }

journal.cli.think.validation.activity.requires.day = { $activity_flag } requiere { $day_flag }

journal.cli.think.validation.activity.requires.facet = { $activity_flag } requiere { $facet_flag }

journal.cli.think.validation.cadence.incompatible = { $cadence_flag } es incompatible con { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag } y { $weekly_flag }

journal.cli.think.validation.daily.only = { $daily_only_flag } requiere { $day_flag } y no se puede combinar con otro modo de razonamiento

journal.cli.think.validation.facet.requires.activity = { $facet_flag } requiere { $activity_flag }

journal.cli.think.validation.flush.incompatible = { $flush_flag } es incompatible con { $segments_flag } y { $refresh_flag }

journal.cli.think.validation.flush.requires.segment = { $flush_flag } requiere { $segment_flag }

journal.cli.think.validation.multi.worker.unlimited.jobs = { $jobs_flag } 0 es incompatible con { $segments_flag } multiproceso; establece { $jobs_flag } en un límite positivo o { $segment_workers_flag } 1

journal.cli.think.validation.no.activity.prompts.with.activity = { $no_activity_prompts_flag } no se puede combinar con { $activity_flag }

journal.cli.think.validation.reactivate.incompatible.refresh = { $reactivate_flag } es incompatible con { $refresh_flag }

journal.cli.think.validation.reactivate.requires.activity = { $reactivate_flag } requiere { $activity_flag }

journal.cli.think.validation.repair.segment = { $repair_segment_flag } requiere { $day_flag } y { $segment_flag } y no se puede combinar con otro modo de razonamiento

journal.cli.think.validation.retry.unit = { $retry_unit_flag } requiere { $day_flag } y { $retry_cause_flag } sin otro modo de razonamiento

journal.cli.think.validation.segment.workers.range = { $segment_workers_flag } debe estar entre 1 y 32

journal.cli.think.validation.segments.incompatible = { $segments_flag } es incompatible con { $segment_flag } y { $facet_flag }

journal.cli.think.validation.settle.incompatible = { $settle_flag } es incompatible con { $segment_flag }, { $segments_flag }, { $activity_flag }, { $flush_flag }, { $refresh_flag }, { $weekly_flag } y { $cadence_flag }

journal.cli.think.validation.weekly.incompatible = { $weekly_flag } es incompatible con { $segment_flag }, { $segments_flag }, { $activity_flag } y { $flush_flag }

journal.cli.think.validation.work.intent = { $work_intent_flag } requiere una solicitud diaria desde cero

journal.cli.think.wait.cortex = falló la espera de cortex: { $error }

journal.cli.think.wait.failed = falló la espera: { $error }

journal.cli.transcribe.audio.directory = El archivo de audio debe estar en un directorio de segmento ({ $pattern }), pero el directorio padre es: { $parent }

journal.cli.transcribe.audio.format = Formato de audio no admitido: { $extension }. Formatos admitidos: { $formats }

journal.cli.transcribe.audio.missing = Archivo de audio no encontrado.{ $line_break }  Se intentó la ruta absoluta:           { $absolute }{ $line_break }  Se intentó la ruta relativa al diario: { $relative }

journal.cli.transcribe.backend.choice = argumento { $backend_flag }: opción no válida: '{ $backend }' (elige entre { $choices })

journal.cli.transcribe.backend.duplicate = argumento { $backend_flag }: solo se puede especificar una vez

journal.cli.transcribe.batch.day =
    { $count ->
        [one] { $verb }: no se pudo leer { $count } día
        [many] { $verb }: no se pudo leer { $count } día(s)
       *[other] { $verb }: no se pudo leer { $count } día(s)
    }

journal.cli.transcribe.batch.day.record = { $verb }: no se pudo leer { $count } día(s)

journal.cli.transcribe.batch.failed = transcripción: { $failed } falló

journal.cli.transcribe.claim.input.cancelled = se canceló la verificación de la entrada de procesamiento

journal.cli.transcribe.claim.input.changed.commit = la entrada de procesamiento cambió antes de la publicación

journal.cli.transcribe.claim.input.changed.open = la entrada de procesamiento cambió al abrir

journal.cli.transcribe.claim.input.changed.read = la entrada de procesamiento cambió al leer

journal.cli.transcribe.claim.input.no.attempt = la entrada no tiene intento de salida

journal.cli.transcribe.claim.input.not.file = la entrada de procesamiento no es un archivo regular

journal.cli.transcribe.claim.lock.entry.unsafe = la entrada del bloqueo persistente es un { $kind } no seguro: { $path }

journal.cli.transcribe.claim.lock.invalid = entrada de bloqueo persistente no válida { $name }

journal.cli.transcribe.claim.lock.missing = falta el directorio padre del bloqueo persistente: { $path }

journal.cli.transcribe.claim.lock.mode = la entrada del bloqueo persistente tiene el modo { $mode }, se esperaba 600: { $path }

journal.cli.transcribe.claim.lock.namespace = la entrada del bloqueo persistente cambió durante la adquisición: { $path }

journal.cli.transcribe.claim.lock.parent.changed = el directorio padre del bloqueo persistente cambió durante la adquisición: { $path }

journal.cli.transcribe.claim.lock.parent.unsafe = el directorio padre del bloqueo persistente es un { $kind } no seguro: { $path }

journal.cli.transcribe.claim.parent.missing = falta el directorio padre

journal.cli.transcribe.claim.root.absent = la raíz del diario no existe

journal.cli.transcribe.claim.root.absolute = la raíz del diario debe ser absoluta

journal.cli.transcribe.claim.root.ancestor = la raíz canónica del diario tiene un ancestro no UTF-8

journal.cli.transcribe.claim.root.canonical.absolute = la raíz canónica del diario no es absoluta

journal.cli.transcribe.claim.root.changed = la fuente del diario cambió durante la adquisición

journal.cli.transcribe.claim.root.component = la raíz canónica del diario no tiene componente final

journal.cli.transcribe.claim.root.directory = la raíz del diario no es un directorio

journal.cli.transcribe.claim.root.invalid = raíz del diario no válida { $path }: { $detail }

journal.cli.transcribe.claim.root.io = { $operation }: { $reason }

journal.cli.transcribe.claim.root.unsupported = raíz del diario no admitida { $path }: { $detail }

journal.cli.transcribe.claim.segment.not.covered = el segmento no está cubierto por estos intentos de salida

journal.cli.transcribe.claim.target.name = falta el nombre de objetivo del bloqueo

journal.cli.transcribe.config.backend = { $key } no válido en la configuración del diario; elige uno de: { $backends }

journal.cli.transcribe.error.asset.absent = no se pudo encontrar el recurso de modelo no vacío { $asset }; se buscó en { $paths }

journal.cli.transcribe.error.asset.digest = el recurso de modelo { $asset } en { $path } tiene { $code_sha256 } { $actual }, se esperaba { $expected }

journal.cli.transcribe.error.asset.executable = no se pudo determinar el ejecutable actual: { $error }

journal.cli.transcribe.error.asset.override = { $code_solstone_transcribe_model_assets_dir }={ $directory } no proporciona el recurso de modelo no vacío { $asset }

journal.cli.transcribe.error.audio.budget = se excedió el presupuesto de muestras decodificadas { $limit } para { $path }

journal.cli.transcribe.error.audio.corrupt = entrada de audio corrupta { $path }: { $detail }

journal.cli.transcribe.error.audio.empty = la entrada de audio está vacía: { $path }

journal.cli.transcribe.error.audio.ffmpeg = { $code_ffmpeg } error para { $path }: { $detail }

journal.cli.transcribe.error.audio.flush =
    { $iterations ->
        [one] { $flushed ->
            [one] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteración ({ $flushed } muestra vaciada; { $remaining } sigue con retraso)
            [many] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteración ({ $flushed } muestras vaciadas; { $remaining } sigue con retraso)
            *[other] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteración ({ $flushed } muestras vaciadas; { $remaining } sigue con retraso)
        }
        [many] { $flushed ->
            [one] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteraciones ({ $flushed } muestra vaciada; { $remaining } sigue con retraso)
            [many] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteraciones ({ $flushed } muestras vaciadas; { $remaining } sigue con retraso)
            *[other] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteraciones ({ $flushed } muestras vaciadas; { $remaining } sigue con retraso)
        }
       *[other] { $flushed ->
            [one] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteraciones ({ $flushed } muestra vaciada; { $remaining } sigue con retraso)
            [many] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteraciones ({ $flushed } muestras vaciadas; { $remaining } sigue con retraso)
            *[other] el vaciado del remuestreador no convergió para { $path } después de { $iterations } iteraciones ({ $flushed } muestras vaciadas; { $remaining } sigue con retraso)
        }
    }

journal.cli.transcribe.error.audio.io = { $code_i_o } error para { $path }: { $error }

journal.cli.transcribe.error.audio.sample.rate = frecuencia de muestreo { $code_wav } no válida: { $rate }

journal.cli.transcribe.error.audio.samples = no se decodificaron muestras de audio de { $path }

journal.cli.transcribe.error.audio.sidecar.create = no se puede crear el sidecar { $code_f32le } { $path }: { $error }

journal.cli.transcribe.error.audio.sidecar.sync = no se puede sincronizar el sidecar { $code_f32le } { $path }: { $error }

journal.cli.transcribe.error.audio.sidecar.write = no se puede escribir el sidecar { $code_f32le } { $path }: { $error }

journal.cli.transcribe.error.audio.stream = no hay flujo de audio en { $path }

journal.cli.transcribe.error.audio.wav.size =
    { $samples ->
        [one] los datos WAV son demasiado grandes para un campo { $riff_u32 }: { $samples } muestra
        [many] los datos WAV son demasiado grandes para un campo { $riff_u32 }: { $samples } muestras
       *[other] los datos WAV son demasiado grandes para un campo { $riff_u32 }: { $samples } muestras
    }

journal.cli.transcribe.error.backend.memory = no hay un backend { $code_stt } viable (memoria disponible: { $available }, piso local: { $floor })

journal.cli.transcribe.error.backend.platform = el backend { $code_stt } no está disponible en esta plataforma anfitriona: { $backend }

journal.cli.transcribe.error.cancelled = se canceló la transcripción incrustada antes de la publicación

journal.cli.transcribe.error.claim = no se pudo adquirir la propiedad de la transcripción para { $path }: { $detail }

journal.cli.transcribe.error.confidential.confidential.audio.handling.is.disabled = el manejo de audio confidencial está desactivado

journal.cli.transcribe.error.confidential.confidential.lane.blocks.stt.backend.backend.raw.audio = el carril confidencial bloquea el backend { $code_stt } { $backend }; el audio en bruto debe permanecer local

journal.cli.transcribe.error.confidential.failure = transcripción confidencial { $reason }: { $detail }

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.status = el { $code_stt } alojado devolvió { $code_http } { $status }

journal.cli.transcribe.error.confidential.hosted.stt.returned.http.unauthorized = el { $code_stt } alojado devolvió { $code_http } 401

journal.cli.transcribe.error.confidential.the.confidential.admission.probe.failed = la sonda de admisión confidencial falló

journal.cli.transcribe.error.confidential.the.confidential.attestation.channel.is.not.ready = el canal de atestación confidencial no está listo

journal.cli.transcribe.error.confidential.the.confidential.endpoint.has.no.credential = el punto de conexión confidencial no tiene credencial

journal.cli.transcribe.error.confidential.the.confidential.lane.has.no.confidential.byo.endpoint = el carril confidencial no tiene un punto de conexión { $code_byo } confidencial

journal.cli.transcribe.error.confidential.the.confidential.lane.is.no.longer.active = el carril confidencial ya no está activo

journal.cli.transcribe.error.confidential.word.a.word.entry.was.blank = una entrada de palabra estaba en blanco

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.a.required.key = a una entrada de palabra le faltaba una clave obligatoria

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.end = a una entrada de palabra le faltaba `end`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.start = a una entrada de palabra le faltaba `start`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.missing.word = a una entrada de palabra le faltaba `word`

journal.cli.transcribe.error.confidential.word.a.word.entry.was.not.an.object = una entrada de palabra no era un objeto

journal.cli.transcribe.error.confidential.word.contract = la respuesta del { $code_stt } alojado violó el contrato verbose de { $code_json }: { $detail }

journal.cli.transcribe.error.confidential.word.response.had.no.words.array = la respuesta no tenía un arreglo de palabras

journal.cli.transcribe.error.confidential.word.response.had.text.but.no.word.timings = la respuesta tenía texto pero no tiempos de palabra

journal.cli.transcribe.error.confidential.word.response.was.not.an.object = la respuesta no era un objeto

journal.cli.transcribe.error.confidential.word.response.was.not.json = la respuesta no era { $code_json }

journal.cli.transcribe.error.confidential.word.word.key.was.not.a.finite.number.found = palabra `{ $key }` no era un número finito (se encontró { $found })

journal.cli.transcribe.error.coreml.failure = { $code_parakeet } { $code_coreml } { $reason }: { $detail }

journal.cli.transcribe.error.coreml.helper.json.was.not.an.object = el { $code_json } del auxiliar no era un objeto

journal.cli.transcribe.error.coreml.helper.response.has.invalid.key = la respuesta del auxiliar tiene { $key } no válido

journal.cli.transcribe.error.coreml.helper.response.missing.key = a la respuesta del auxiliar le falta { $key }

journal.cli.transcribe.error.coreml.helper.response.missing.token.timings = a la respuesta del auxiliar le falta { $code_token_timings }

journal.cli.transcribe.error.coreml.helper.returned.transcript.text.without.token.timings = el auxiliar devolvió texto de la transcripción sin tiempos de token

journal.cli.transcribe.error.coreml.helper.token.timings.collapsed.to.no.words = los tiempos de token del auxiliar colapsaron sin dejar palabras

journal.cli.transcribe.error.coreml.helper.version.json.was.not.an.object = el { $code_json } de la versión del auxiliar no era un objeto

journal.cli.transcribe.error.coreml.helper.version.response.missing.field = a la respuesta de la versión del auxiliar le falta { $field }

journal.cli.transcribe.error.coreml.missing.stderr.pipe = falta la tubería { $code_stderr }

journal.cli.transcribe.error.coreml.missing.stdout.pipe = falta la tubería { $code_stdout }

journal.cli.transcribe.error.coreml.name.reader.panicked = { $name } el lector entró en pánico

journal.cli.transcribe.error.coreml.parakeet.coreml.model.is.not.installed.yet.path = { $code_parakeet } { $code_coreml } el modelo todavía no está instalado: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.exited.with.status = { $code_parakeet } el auxiliar terminó con { $status }

journal.cli.transcribe.error.coreml.parakeet.helper.is.missing.path = { $code_parakeet } falta el auxiliar: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.a.file.path = { $code_parakeet } el auxiliar no es un archivo: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.is.not.executable.path = { $code_parakeet } el auxiliar no es ejecutable: { $path }

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out = { $code_parakeet } se agotó el tiempo del auxiliar

journal.cli.transcribe.error.coreml.parakeet.helper.timed.out.after.seconds.s = { $code_parakeet } se agotó el tiempo del auxiliar después de { $seconds }s

journal.cli.transcribe.error.coreml.publication.busy = { $code_parakeet } { $code_coreml } el bloqueo de publicación está ocupado

journal.cli.transcribe.error.coreml.token.timing.must.be.an.object = el tiempo de token debe ser un objeto

journal.cli.transcribe.error.cpp.device.must.be.one.of.auto.cpu = el dispositivo debe ser uno de: { $code_auto_cpu }

journal.cli.transcribe.error.cpp.failure = { $code_parakeet_cpp } { $reason }: { $detail }

journal.cli.transcribe.error.cpp.http.status = { $code_http } { $status }

journal.cli.transcribe.error.cpp.parakeet.cpp.is.only.supported.on.linux.and = { $code_parakeet_cpp } solo se admite en linux y windows

journal.cli.transcribe.error.cpp.parakeet.server.credentials.are.unavailable.in.this.process = { $code_parakeet } las credenciales del servidor no están disponibles en este proceso.

journal.cli.transcribe.error.cpp.parakeet.server.is.not.ready.yet = { $code_parakeet } el servidor todavía no está listo.

journal.cli.transcribe.error.cpp.response.has.text.but.no.word.timings = la respuesta tiene texto pero no tiempos de palabra

journal.cli.transcribe.error.cpp.response.json.was.invalid.error = el { $code_json } de la respuesta no era válido: { $error }

journal.cli.transcribe.error.cpp.response.json.was.not.an.object = el { $code_json } de la respuesta no era un objeto

journal.cli.transcribe.error.cpp.response.missing.top.level.words = a la respuesta le falta { $code_words } de nivel superior

journal.cli.transcribe.error.cpp.tls.is.required = { $code_tls } es obligatorio

journal.cli.transcribe.error.cpp.transcription.request.timed.out = se agotó el tiempo de la solicitud de transcripción

journal.cli.transcribe.error.cpp.word.timing.contains.invalid.numeric.value = el tiempo de palabra contiene un valor numérico no válido

journal.cli.transcribe.error.cpp.word.timing.item.must.be.an.object = el elemento del tiempo de palabra debe ser un objeto

journal.cli.transcribe.error.cpp.word.timing.missing.key.key = al tiempo de palabra le falta la clave: { $key }

journal.cli.transcribe.error.cpp.word.timing.text.was.blank = el texto del tiempo de palabra estaba en blanco

journal.cli.transcribe.error.input.inspect = no se pudo inspeccionar la entrada { $path }: { $detail }

journal.cli.transcribe.error.orphan = no se pudo quitar el { $code_npz } huérfano { $path }: { $detail }

journal.cli.transcribe.error.payload.parent = el destino de la transcripción no tiene directorio padre

journal.cli.transcribe.error.process.admission = falló la admisión del lanzamiento alojado: { $reason }

journal.cli.transcribe.error.process.capability = la capacidad del anfitrión no está disponible: { $needed }

journal.cli.transcribe.error.process.command = no se puede lanzar un comando vacío

journal.cli.transcribe.error.process.confirm = falló la confirmación posterior al lanzamiento para el pid { $pid }: { $error }

journal.cli.transcribe.error.process.explicit = esta autoridad no está explícitamente sin dueño

journal.cli.transcribe.error.process.identity = no se pudo capturar la identidad ligada al nacimiento del pid lanzado { $pid }

journal.cli.transcribe.error.process.log = no se pudo preparar el registro operativo: { $error }

journal.cli.transcribe.error.process.output = la salida del proceso hijo no está disponible

journal.cli.transcribe.error.process.spawn = no se pudo lanzar el proceso hijo: { $error }

journal.cli.transcribe.error.process.terminate = no se pudo terminar el proceso hijo: { $error }

journal.cli.transcribe.error.process.unowned.reason = { $code_explicitlyunowned } el motivo no debe estar vacío

journal.cli.transcribe.error.publish.output = { $code_output_jsonl_path } es obligatorio

journal.cli.transcribe.error.publish.parent = la salida no tiene directorio padre

journal.cli.transcribe.error.publish.raw = { $code_header_raw } es obligatorio

journal.cli.transcribe.error.publish.sibling = el raw de procesamiento debe ser un nombre de archivo hermano

journal.cli.transcribe.error.speaker.analysis = falló el análisis de hablante: { $stage }/{ $reason }

journal.cli.transcribe.error.statement.field = el enunciado de la transcripción completa no tiene un valor válido para { $field }

journal.cli.transcribe.error.terminal.payload = no se pudo preparar la carga terminal: { $detail }

journal.cli.transcribe.error.terminal.payload.path = no se pudo preparar o quitar la carga terminal { $path }: { $detail }

journal.cli.transcribe.error.terminal.request = no se pudo serializar la solicitud del escritor terminal: { $detail }

journal.cli.transcribe.error.transcript.payload = no se pudo preparar la carga de la transcripción: { $detail }

journal.cli.transcribe.error.transcript.payload.path = no se pudo preparar o quitar la carga de la transcripción { $path }: { $detail }

journal.cli.transcribe.error.transcript.request = no se pudo serializar la solicitud del escritor de la transcripción: { $detail }

journal.cli.transcribe.error.vad.contract.exit = { $code_vad } error de contrato del auxiliar (salida { $code }): { $detail }

journal.cli.transcribe.error.vad.contract.exit.stderr = { $code_vad } error de contrato del auxiliar (salida { $code }): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.contract.signal = { $code_vad } error de contrato del auxiliar (sin código de salida): { $detail }

journal.cli.transcribe.error.vad.contract.signal.stderr = { $code_vad } error de contrato del auxiliar (sin código de salida): { $detail }: { $stderr }

journal.cli.transcribe.error.vad.could.not.create.vad.input.directory.error = no se pudo crear el directorio de entrada de { $code_vad }: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.error = no se pudo quitar el directorio de entrada de { $code_vad }: { $error }

journal.cli.transcribe.error.vad.could.not.remove.vad.input.directory.input.remains.in = no se pudo quitar el directorio de entrada de { $code_vad }: la entrada sigue en uso

journal.cli.transcribe.error.vad.could.not.send.vad.request.error = no se pudo enviar la solicitud de { $code_vad }: { $error }

journal.cli.transcribe.error.vad.could.not.serialize.vad.request.error = no se pudo serializar la solicitud de { $code_vad }: { $error }

journal.cli.transcribe.error.vad.error.empty = { $code_vad } la respuesta de error estaba vacía

journal.cli.transcribe.error.vad.error.many = { $code_vad } la respuesta de error contenía más de una línea

journal.cli.transcribe.error.vad.error.utf8 = { $code_vad } la respuesta de error no era { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.helper = { $code_vad } el auxiliar { $reason } (salida { $code }): { $detail }

journal.cli.transcribe.error.vad.launch.failed = no se pudo lanzar el auxiliar { $code_vad } { $path }: { $detail }

journal.cli.transcribe.error.vad.response.empty = { $code_vad } la respuesta estaba vacía

journal.cli.transcribe.error.vad.response.many = { $code_vad } la respuesta contenía más de una línea

journal.cli.transcribe.error.vad.response.utf8 = { $code_vad } la respuesta no era { $code_utf_8 }: { $error }

journal.cli.transcribe.error.vad.vad.error.response.lacks.schema.reason.or.detail = { $code_vad } la respuesta de error no tiene esquema, motivo ni detalle

journal.cli.transcribe.error.vad.vad.error.response.must.be.an.object = { $code_vad } la respuesta de error debe ser un objeto

journal.cli.transcribe.error.vad.vad.error.response.violates.its.schema = { $code_vad } la respuesta de error viola su esquema

journal.cli.transcribe.error.vad.vad.error.response.was.not.json.error = { $code_vad } la respuesta de error no era { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.helper.stdin.was.unavailable = { $code_vad } el { $code_stdin } del auxiliar no estaba disponible

journal.cli.transcribe.error.vad.vad.helper.terminated.without.an.exit.code = { $code_vad } el auxiliar terminó sin código de salida

journal.cli.transcribe.error.vad.vad.response.has.an.unknown.schema = { $code_vad } la respuesta tiene un esquema desconocido

journal.cli.transcribe.error.vad.vad.response.has.no.boolean.has.speech = { $code_vad } la respuesta no tiene el booleano { $code_has_speech }

journal.cli.transcribe.error.vad.vad.response.has.no.non.empty.field = { $code_vad } la respuesta no tiene un valor no vacío en { $field }

journal.cli.transcribe.error.vad.vad.response.has.no.non.negative.finite.field = { $code_vad } la respuesta no tiene un valor finito no negativo en { $field }

journal.cli.transcribe.error.vad.vad.response.has.no.speech.array = { $code_vad } la respuesta no tiene un arreglo de habla

journal.cli.transcribe.error.vad.vad.response.must.be.an.object = { $code_vad } la respuesta debe ser un objeto

journal.cli.transcribe.error.vad.vad.response.was.not.json.error = { $code_vad } la respuesta no era { $code_json }: { $error }

journal.cli.transcribe.error.vad.vad.speech.entry.ends.before.it.starts = { $code_vad } la entrada de habla termina antes de empezar

journal.cli.transcribe.error.vad.vad.speech.entry.has.no.valid.field.sample.index = { $code_vad } la entrada de habla no tiene un índice de muestra válido en { $field }

journal.cli.transcribe.error.vad.vad.speech.entry.must.be.an.object = { $code_vad } la entrada de habla debe ser un objeto

journal.cli.transcribe.error.vad.wait.failed = no se pudo esperar al auxiliar { $code_vad }: { $detail }

journal.cli.transcribe.error.vad.write = no se pudo escribir la entrada de { $code_vad }: { $detail }

journal.cli.transcribe.error.voice.wait = la transcripción de solo voz está en espera: { $detail }

journal.cli.transcribe.error.windows.admission.deadline.exceeded = la admisión del auxiliar acotado no terminó antes de su plazo

journal.cli.transcribe.error.windows.blocked = { $detail }; la limpieza previa del auxiliar bloquea la admisión: { $blockers }

journal.cli.transcribe.error.windows.cancelled = se canceló el auxiliar acotado

journal.cli.transcribe.error.windows.cleanup = { $detail }; la limpieza estaba incompleta al retornar: { $cleanup }

journal.cli.transcribe.error.windows.cleanup.blocked = { $detail }; la limpieza estaba incompleta al retornar: { $cleanup }; la limpieza previa del auxiliar bloquea la admisión: { $blockers }

journal.cli.transcribe.error.windows.cleanup.contended = la admisión de la limpieza del auxiliar acotado está en contención

journal.cli.transcribe.error.windows.cleanup.pending = la limpieza anterior del auxiliar acotado bloquea la admisión

journal.cli.transcribe.error.windows.current.directory.not.directory = el directorio actual del auxiliar acotado no es un directorio

journal.cli.transcribe.error.windows.current.directory.outside.package = el directorio actual del auxiliar acotado está fuera de la raíz de su paquete

journal.cli.transcribe.error.windows.current.directory.unavailable = no se pudo canonizar el directorio actual del auxiliar acotado

journal.cli.transcribe.error.windows.deadline.exceeded = el auxiliar acotado no terminó antes de su plazo

journal.cli.transcribe.error.windows.executable.not.file = el ejecutable del auxiliar acotado no es un archivo regular

journal.cli.transcribe.error.windows.executable.outside.package = el ejecutable del auxiliar acotado está fuera de la raíz de su paquete

journal.cli.transcribe.error.windows.executable.unavailable = no se pudo canonizar el ejecutable del auxiliar acotado

journal.cli.transcribe.error.windows.input.limit.exceeded = la entrada del auxiliar acotado supera su límite de bytes declarado

journal.cli.transcribe.error.windows.input.write.failed = falló el escritor de entrada del auxiliar acotado

journal.cli.transcribe.error.windows.io.worker.start.failed = no se pudo iniciar el proceso de { $code_i_o } de { $stream } del auxiliar acotado

journal.cli.transcribe.error.windows.job.not.quiescent = la finalización del auxiliar acotado no estableció la quiescencia del Job

journal.cli.transcribe.error.windows.launch.failed = el auxiliar acotado falló antes de que estuviera disponible una identidad propia del proceso hijo

journal.cli.transcribe.error.windows.launch.finalization.failed = falló la finalización del lanzamiento nativo: { $detail }

journal.cli.transcribe.error.windows.missing.system.root = el entorno del auxiliar acotado debe contener { $code_systemroot } no vacío

journal.cli.transcribe.error.windows.output.limit.exceeded = la salida { $stream } del auxiliar acotado superó su límite de bytes declarado

journal.cli.transcribe.error.windows.output.read.failed = falló el lector de salida { $stream } del auxiliar acotado

journal.cli.transcribe.error.windows.package.root.unavailable = no se pudo canonizar la raíz del paquete del auxiliar acotado

journal.cli.transcribe.error.windows.path.environment.refused = el entorno del auxiliar acotado no puede contener { $code_path }

journal.cli.transcribe.error.windows.path.not.representable = la ruta del auxiliar acotado no se puede representar en el límite administrado de comandos de Windows

journal.cli.transcribe.error.windows.process.observation.failed = falló la observación del proceso del auxiliar acotado

journal.cli.transcribe.error.windows.zero.limit = el límite de bytes de { $stream } del auxiliar acotado debe ser distinto de cero

journal.cli.transcribe.error.windows.zero.timeout = el tiempo de espera del auxiliar acotado debe ser distinto de cero

journal.cli.transcribe.error.writer.base.time.us.of.day.is.outside.a.day = { $code_base_time_us_of_day } está fuera de un día

journal.cli.transcribe.error.writer.byte.count.does.not.match.shape = { $code_byte_count } no coincide con la forma

journal.cli.transcribe.error.writer.byte.count.is.too.large = { $code_byte_count } es demasiado grande

journal.cli.transcribe.error.writer.cancelled = la publicación de la transcripción se canceló antes de la admisión

journal.cli.transcribe.error.writer.destination = el destino ya existe: { $path }

journal.cli.transcribe.error.writer.dtype.must.be.float32.le = { $code_dtype } debe ser { $code_float32_le }

journal.cli.transcribe.error.writer.duration.and.noisy.must.be.provided.together = duration y noisy deben proporcionarse juntos

journal.cli.transcribe.error.writer.durations.s.count.does.not.match.shape = la cantidad de { $code_durations_s } no coincide con la forma

journal.cli.transcribe.error.writer.durations.s.must.be.finite.f32.values = { $code_durations_s } debe tener valores { $code_f32 } finitos

journal.cli.transcribe.error.writer.encoder.must.be.nonempty = el codificador no debe estar vacío

journal.cli.transcribe.error.writer.field.must.be.a.boolean = { $field } debe ser un booleano

journal.cli.transcribe.error.writer.field.must.be.a.finite.number = { $field } debe ser un número finito

journal.cli.transcribe.error.writer.field.must.be.a.string = { $field } debe ser una cadena

journal.cli.transcribe.error.writer.field.must.be.an.array = { $field } debe ser un arreglo

journal.cli.transcribe.error.writer.field.must.be.an.integer = { $field } debe ser un entero

journal.cli.transcribe.error.writer.field.must.be.an.unsigned.integer = { $field } debe ser un entero sin signo

journal.cli.transcribe.error.writer.header.numbers.must.be.finite = los números del encabezado deben ser finitos

journal.cli.transcribe.error.writer.id.range = el id debe estar en { $code_1_i32_max }

journal.cli.transcribe.error.writer.invalid.f32.payload.chunk = fragmento de carga { $code_f32 } no válido

journal.cli.transcribe.error.writer.jsonl.path.and.npz.path.must.be.sibling.files = { $code_jsonl_path } y { $code_npz_path } deben ser archivos hermanos que compartan el nombre sin extensión

journal.cli.transcribe.error.writer.missing.required.field.key = falta el campo obligatorio { $key }

journal.cli.transcribe.error.writer.noisy.s.is.required.when.noisy.rms.is.present = { $code_noisy_s } es obligatorio cuando { $code_noisy_rms } está presente

journal.cli.transcribe.error.writer.npz.cancelled = se canceló

journal.cli.transcribe.error.writer.npz.count = { $code_npz } tiene una cantidad de miembros inesperada

journal.cli.transcribe.error.writer.npz.members = los miembros de { $code_npz } no coinciden con el contrato del archivo complementario de la transcripción

journal.cli.transcribe.error.writer.npz.missing = falta el miembro { $name } de { $code_npz }

journal.cli.transcribe.error.writer.npz.parent = el destino npz no tiene directorio padre

journal.cli.transcribe.error.writer.npz.readback = el miembro { $name } de { $code_npz } no coincidió al releerlo

journal.cli.transcribe.error.writer.output.path.has.no.parent = la ruta de salida no tiene directorio padre

journal.cli.transcribe.error.writer.output.paths.must.be.nonempty = las rutas de salida no deben estar vacías

journal.cli.transcribe.error.writer.payload.finite = la carga de incrustación no es finita en la fila { $row }, columna { $column }

journal.cli.transcribe.error.writer.payload.format.must.be.raw.f32le.row.major.v1 = { $code_payload_format } debe ser { $code_raw_f32le_row_major_v1 }

journal.cli.transcribe.error.writer.payload.size.does.not.match.byte.count = el tamaño de la carga no coincide con { $code_byte_count }

journal.cli.transcribe.error.writer.prior.jsonl.snapshot.is.missing = falta la instantánea jsonl anterior

journal.cli.transcribe.error.writer.prior.npz.snapshot.is.missing = falta la instantánea npz anterior

journal.cli.transcribe.error.writer.prior.restore.refused = se rechazó la restauración anterior

journal.cli.transcribe.error.writer.private.stage.is.missing = falta la preparación privada

journal.cli.transcribe.error.writer.private.transcript.copy.did.not.match.the.staged.file = la copia privada de la transcripción no coincidió con el archivo en preparación

journal.cli.transcribe.error.writer.private.transcript.jsonl.is.missing = falta el jsonl privado de la transcripción

journal.cli.transcribe.error.writer.rounded.header.number.is.not.json.representable = el número redondeado del encabezado no es representable en { $code_json }

journal.cli.transcribe.error.writer.schema = esquema desconocido: { $schema }

journal.cli.transcribe.error.writer.segment.meta.must.be.an.object = { $code_segment_meta } debe ser un objeto

journal.cli.transcribe.error.writer.shape.byte.count.overflows = la cantidad de bytes de la forma se desborda

journal.cli.transcribe.error.writer.shape.must.be.row.count.width = la forma debe ser { $code_row_count_256 }

journal.cli.transcribe.error.writer.shape.row.count.is.too.large = la cantidad de filas de la forma es demasiado grande

journal.cli.transcribe.error.writer.speaker.evidence.multi.fraction.is.required.with.speaker.evidence = { $code_speaker_evidence_multi_fraction } es obligatorio con { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.evidence.version.is.required.with.speaker.evidence = { $code_speaker_evidence_version } es obligatorio con { $code_speaker_evidence }

journal.cli.transcribe.error.writer.speaker.must.be.a.string.integer.null.or.absent = el hablante debe ser una cadena, un entero, null o ausente

journal.cli.transcribe.error.writer.speech.loud.windows.is.required.when.loud.windows.is = { $code_speech_loud_windows } es obligatorio cuando { $code_loud_windows } es positivo

journal.cli.transcribe.error.writer.stage.path.already.exists = la ruta de la preparación ya existe

journal.cli.transcribe.error.writer.stage.path.is.a.symlink = la ruta de preparación es un enlace simbólico

journal.cli.transcribe.error.writer.stage.path.is.not.a.private.directory = la ruta de preparación no es un directorio privado

journal.cli.transcribe.error.writer.stale.npz.unlink.refused = se rechazó desvincular el npz desactualizado

journal.cli.transcribe.error.writer.statement = enunciado { $index }: { $detail }

journal.cli.transcribe.error.writer.statement.duplicate = el enunciado { $index } duplica el id { $id }

journal.cli.transcribe.error.writer.statement.id.is.not.a.transcript.statement.id = el ID de enunciado no es un ID de enunciado de transcripción

journal.cli.transcribe.error.writer.statement.id.is.outside.i32 = el ID de enunciado está fuera de { $code_i32 }

journal.cli.transcribe.error.writer.statement.id.missing = al enunciado { $index } le falta el id

journal.cli.transcribe.error.writer.statement.ids.count.does.not.match.shape = la cantidad de { $code_statement_ids } no coincide con la forma

journal.cli.transcribe.error.writer.statement.ids.must.be.unique = los IDs de enunciado deben ser únicos

journal.cli.transcribe.error.writer.statement.text.ended.before.its.trimmed.length = el texto del enunciado terminó antes de su longitud recortada

journal.cli.transcribe.error.writer.statement.text.is.not.valid.utf8 = el texto del enunciado no es { $code_utf_8 } válido

journal.cli.transcribe.error.writer.text.must.be.a.nonempty.string = el texto debe ser una cadena no vacía

journal.cli.transcribe.error.writer.turn.audio.duration.s.must.be.non.negative = { $code_audio_duration_s } debe ser no negativo

journal.cli.transcribe.error.writer.turn.boundary.suspect.must.be.true.if.present = { $code_turn_boundary_suspect } debe ser verdadero si está presente

journal.cli.transcribe.error.writer.turn.expected.version.expected.got.version = se esperaba la versión { $expected }, se obtuvo { $version }

journal.cli.transcribe.error.writer.turn.field.is.missing = falta { $field }

journal.cli.transcribe.error.writer.turn.field.must.be.finite = { $field } debe ser finito

journal.cli.transcribe.error.writer.turn.invalid = { $code_speaker_turn_evidence } no válido: { $detail }

journal.cli.transcribe.error.writer.turn.range.index.end.s.end.s.exceeds.audio = el rango { $index } { $code_end_s } { $end_s } supera { $code_audio_duration_s } { $audio_duration_s }

journal.cli.transcribe.error.writer.turn.range.index.has.unexpected.keys = el rango { $index } tiene claves inesperadas

journal.cli.transcribe.error.writer.turn.range.index.is.an.exact.duplicate = el rango { $index } es un duplicado exacto

journal.cli.transcribe.error.writer.turn.range.index.is.inverted.start.s.start.s = el rango { $index } está invertido: { $code_start_s } { $start_s } > { $code_end_s } { $end_s }

journal.cli.transcribe.error.writer.turn.range.index.must.be.an.object = el rango { $index } debe ser un objeto

journal.cli.transcribe.error.writer.turn.range.index.start.s.must.be.non.negative = el rango { $index } { $code_start_s } debe ser no negativo

journal.cli.transcribe.error.writer.turn.ranges.must.be.an.array = los rangos deben ser un arreglo

journal.cli.transcribe.error.writer.turn.ranges.must.be.sorted.range.index.is.out = los rangos deben estar ordenados: el rango { $index } está fuera de orden

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.has.unexpected.keys = la evidencia de turnos de hablante tiene claves inesperadas

journal.cli.transcribe.error.writer.turn.speaker.turns.evidence.must.be.an.object = la evidencia de turnos de hablante debe ser un objeto

journal.cli.transcribe.error.writer.turn.version.must.be.a.string = la versión debe ser una cadena

journal.cli.transcribe.error.writer.zip.central.extra.at.extra.at.error = extra central en { $extra_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.header.at.central.header.start.error = encabezado central en { $central_header_start }: { $error }

journal.cli.transcribe.error.writer.zip.central.name.at.name.at.error = nombre central en { $name_at }: { $error }

journal.cli.transcribe.error.writer.zip.central.record.extends.beyond.archive = el registro central se extiende más allá del archivo comprimido

journal.cli.transcribe.error.writer.zip.central.signature.at.central.header.start = firma central en { $central_header_start }

journal.cli.transcribe.error.writer.zip.cp437.scalar = { $code_cp437 } escalar

journal.cli.transcribe.error.writer.zip.flagged.name.is.not.utf8 = el nombre marcado no es { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.local.header.at.header.start.error = encabezado local en { $header_start }: { $error }

journal.cli.transcribe.error.writer.zip.local.signature.at.header.start = firma local en { $header_start }

journal.cli.transcribe.error.writer.zip.raw.and.exposed.central.populations.disagree = las poblaciones centrales en bruto y expuestas no coinciden

journal.cli.transcribe.error.writer.zip.raw.central.member.was.lost.from.the.zip = se perdió el miembro central en bruto del índice ZIP

journal.cli.transcribe.error.writer.zip.repeated.exposed.central.coordinate = coordenada central expuesta repetida

journal.cli.transcribe.error.writer.zip.unicode.path.extra.crc.mismatch = no coincide el crc del extra de ruta unicode

journal.cli.transcribe.error.writer.zip.unicode.path.extra.is.not.utf8 = el extra de ruta unicode no es { $code_utf_8 }

journal.cli.transcribe.error.writer.zip.unicode.path.extra.too.small = el extra de ruta unicode es demasiado pequeño

journal.cli.transcribe.error.writer.zip.unproved = nombre del miembro zip { $index } no probado: { $detail }

journal.cli.transcribe.error.writer.zip.unreadable = archivo comprimido zip ilegible: { $detail }

journal.cli.transcribe.error.writer.zip.zip.member.name.collision.name = colisión de nombre de miembro zip: { $name }

journal.cli.transcribe.feed.failure = falló la ingesta de la reserva de candidatos de hablante para { $path }: { $detail }

journal.cli.transcribe.feed.parse = no se pudo analizar la transcripción: { $detail }

journal.cli.transcribe.feed.pool.lock = falló el bloqueo de la reserva de candidatos: { $detail }

journal.cli.transcribe.feed.pool.read = falló la lectura de la reserva de candidatos: { $detail }

journal.cli.transcribe.feed.pool.width = el ancho del centroide del conjunto de candidatos no coincide

journal.cli.transcribe.feed.pool.write = falló la escritura de la reserva de candidatos: { $detail }

journal.cli.transcribe.feed.read = no se pudo leer la transcripción: { $detail }

journal.cli.transcribe.feed.unreadable = no se pudo leer la transcripción completada o las incrustaciones

journal.cli.transcribe.file.failed = { $file }: falló la transcripción ({ $reason }); detalles: { $output }

journal.cli.transcribe.help = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $line_break }Transcribir archivos de audio mediante { $stt } intercambiable y análisis nativo de hablante{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }  { $input }            Ruta al archivo de audio en el directorio de segmento del diario, p. ej.{ $line_break }                        { $sample }{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }mostrar este mensaje de ayuda y salir{ $line_break }{ $indent_2 }{ $all_option }{ $gap_17 }Transcribir por lotes todos los segmentos de audio sin procesar en el{ $line_break }                        diario{ $line_break }{ $indent_2 }{ $redo_option }{ $gap_16 }Reprocesar el archivo, sobrescribiendo las salidas existentes{ $line_break }{ $syntax_continuation_16 }{ $line_break }                        { $stt } como backend (anula la configuración y el{ $line_break }                        predeterminado automático según los recursos){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }Activar la salida detallada{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }Activar el registro de depuración

journal.cli.transcribe.installation.capability.mismatch = la capacidad de generación no coincide con la instalación actual

journal.cli.transcribe.installation.coded = { $code }: { $detail }

journal.cli.transcribe.installation.fd.platform = { $fd }: plataforma no compatible

journal.cli.transcribe.installation.grant.kind = tipo de concesión de generación inesperado

journal.cli.transcribe.installation.grant.required = la concesión de generación autenticada falta o está duplicada

journal.cli.transcribe.installation.helper.parent = { $helper }: el ejecutable actual no tiene directorio padre

journal.cli.transcribe.installation.id.encoding = ID de generación no-{ $unicode }

journal.cli.transcribe.installation.lock.kind = el bloqueo de generación no es un archivo ordinario

journal.cli.transcribe.installation.metadata.malformed = metadatos de generación malformados

journal.cli.transcribe.installation.metadata.partial = metadatos de generación parciales

journal.cli.transcribe.installation.metadata.required = faltan los metadatos de generación

journal.cli.transcribe.installation.metadata.unverified = los metadatos de generación no tienen capacidad de lanzamiento autenticada

journal.cli.transcribe.installation.obsolete = marcador obsoleto del descriptor de generación de { $windows }

journal.cli.transcribe.installation.platform = { $code }: { $platform }/{ $architecture }

journal.cli.transcribe.installation.provenance = la procedencia de lanzamiento de la generación no coincide con esta raíz

journal.cli.transcribe.installation.stamp = { $code }: { $path }: { $error }

journal.cli.transcribe.installation.token.encoding = token de generación no-{ $unicode }

journal.cli.transcribe.owner.running = Tu diario está en ejecución y transcribe las grabaciones por sí mismo. trae la grabación con '{ $import }' en su lugar.

journal.cli.transcribe.owner.stopped = Tu diario no está en ejecución. inícialo con '{ $up }', luego trae la grabación con '{ $import }'; tu diario la transcribe.

journal.cli.transcribe.selection.conflict = { $all_flag } y { $input } son mutuamente excluyentes

journal.cli.transcribe.selection.missing = proporciona { $input } o { $all_flag }

journal.cli.transcribe.sound.all.windows.failed = falló el etiquetado de sonido para todas las ventanas: { $detail }

journal.cli.transcribe.sound.engine.absent = el motor ced no está instalado

journal.cli.transcribe.sound.failed = falló el etiquetado de sonido: { $error }

journal.cli.transcribe.sound.helper.executable = el binario del auxiliar ced no es ejecutable: { $path }

journal.cli.transcribe.sound.helper.exit = el auxiliar ced terminó con { $code }: { $stderr }

journal.cli.transcribe.sound.helper.io = falló la E/S del auxiliar ced: { $detail }

journal.cli.transcribe.sound.helper.large = la respuesta de ced superó 64 MiB

journal.cli.transcribe.sound.helper.missing = falta el binario del auxiliar ced: { $path }

journal.cli.transcribe.sound.helper.parent = el ejecutable actual no tiene directorio padre

journal.cli.transcribe.sound.helper.response = la respuesta del auxiliar ced no era JSON válido: { $detail }

journal.cli.transcribe.sound.helper.spawn = no se pudo iniciar el auxiliar ced: { $detail }

journal.cli.transcribe.sound.helper.timeout = el auxiliar ced no terminó dentro de { $timeout }

journal.cli.transcribe.sound.helper.unresolved = auxiliar ced sin resolver: { $detail }

journal.cli.transcribe.sound.helper.windows = CED requiere el llamador del host de Windows

journal.cli.transcribe.sound.package.platform = plataforma CED no compatible { $os }/{ $arch }

journal.cli.transcribe.sound.package.windows.runtime = la verificación del paquete CED de Windows requiere un entorno de ejecución de Windows

journal.cli.transcribe.sound.prepare = no se pudo preparar el audio para el etiquetado de sonido: { $error }

journal.cli.transcribe.sound.probe.helper.exit = el auxiliar de sondeo ced terminó con { $code }: { $stderr }

journal.cli.transcribe.sound.probe.helper.unexpected = el auxiliar de sondeo ced devolvió una respuesta inesperada: { $response }

journal.cli.transcribe.sound.probe.unexpected = el sondeo ced devolvió una respuesta inesperada: { $response }

journal.cli.transcribe.sound.schema = el etiquetado de sonido devolvió un esquema de respuesta inesperado

journal.cli.transcribe.sound.unsupported = etiquetador de sonido desactivado: recursos ced no compatibles en { $os }/{ $arch }

journal.cli.transcribe.sound.window.count = el etiquetado de sonido devolvió una cantidad de ventanas inesperada

journal.cli.transcribe.sound.window.ok = el etiquetado de sonido devolvió una ventana sin un campo { $field }

journal.cli.transcribe.sound.windows.launch = falló el lanzamiento del auxiliar CED: { $error }

journal.cli.transcribe.sound.windows.library = la biblioteca de la solicitud CED no coincide con el paquete firmado

journal.cli.transcribe.sound.windows.package.bin = el ejecutable en ejecución no está en el directorio bin del paquete

journal.cli.transcribe.sound.windows.package.member = la carga útil firmada de la aplicación CED rechaza { $member }: { $error }

journal.cli.transcribe.sound.windows.package.parent = el ejecutable en ejecución no tiene un directorio que lo contenga

journal.cli.transcribe.sound.windows.package.root = el directorio bin del paquete no tiene una raíz de paquete

journal.cli.transcribe.sound.windows.probe = el auxiliar CED devolvió una respuesta de sondeo no válida

journal.cli.transcribe.sound.windows.root = no se estableció { $variable }

journal.cli.transcribe.sound.windows.signed = Windows CED requiere el auxiliar del paquete firmado y su comando declarado

journal.cli.transcribe.stage.confidential.cap = el audio dura { $seconds }s y enviaría { $bytes } bytes; el STT confidencial acepta como máximo { $limit } bytes

journal.cli.transcribe.stage.confidential.verification.clear = no se borró la verificación de la transcripción confidencial para { $path }: { $error }

journal.cli.transcribe.stage.confidential.verification.record = no se registró la verificación de la transcripción confidencial para { $path }: { $error }

journal.cli.transcribe.stage.embedded.missing.draft = la pasada de reconocimiento no tiene spool de borrador

journal.cli.transcribe.stage.embedded.range.missing = falta el rango de turno fusionado

journal.cli.transcribe.stage.embedded.scratch = no se pudo crear el espacio temporal embebido: { $error }

journal.cli.transcribe.stage.output.parent = la ruta de la transcripción no tiene padre

journal.cli.transcribe.stage.owner.deleted.unrecorded = se eliminó el original de solo voz, pero no se registró el evento de eliminación

journal.cli.transcribe.stage.owner.helper.outdated = el auxiliar de análisis de hablante es anterior a rangos de solapamiento o turnos de hablante

journal.cli.transcribe.stage.owner.not.deleted = el original de solo voz no se eliminó ahora: { $outcome }

journal.cli.transcribe.stage.owner.save.or.disable = guarda una voz en Hablantes, o desactiva { $setting }

journal.cli.transcribe.stage.owner.save.to.finish = guarda una voz en Hablantes para terminar esta grabación filtrada

journal.cli.transcribe.summary = { $processed } se procesaron, { $skipped } se omitieron (ya se transcribieron)

journal.cli.transcribe.summary.deferred = { $processed } se procesaron, { $skipped } se omitieron (ya se transcribieron), { $deferred } se aplazaron (el proveedor no está listo, se reintentará)

journal.cli.transcribe.summary.deferred.failed = { $processed } se procesaron, { $skipped } se omitieron (ya se transcribieron), { $deferred } se aplazaron (el proveedor no está listo, se reintentará), { $failed } fallaron

journal.cli.transcribe.summary.failed = { $processed } se procesaron, { $skipped } se omitieron (ya se transcribieron), { $failed } fallaron

journal.cli.transcribe.usage.error = uso: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $syntax_continuation_3 }{ $line_break }{ $command }: error: { $message }
