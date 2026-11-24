journal.cli.about.os.release.unavailable = about: { $release_file } no está disponible: { $error }

journal.cli.about.resource.unavailable = no se pudo leer la información de about del diario. revisa que tu diario esté en ejecución, y luego vuelve a intentarlo.

journal.cli.about.usage.journal = uso: { $syntax }

journal.cli.about.usage.remote = uso: { $syntax }

journal.cli.check.disk.error = no se pudo verificar el espacio libre en { $path }: { $message }

journal.cli.check.disk.free = { $gb } GB libres en { $path } (se necesitan 20 GB)

journal.cli.check.disk.low = el diario y los modelos locales necesitan al menos 20 GB libres — { $path } tiene { $gb } GB

journal.cli.check.disk.overflow = desbordamiento del espacio libre

journal.cli.check.feedback = ¿Crees que esta lectura está mal para tu computadora? Nos encantaría un parche — { $url }

journal.cli.check.gpu.absent = no se encontró una GPU utilizable — los modelos locales incluidos necesitan una GPU de hardware

journal.cli.check.gpu.absent.linux = no se encontró una GPU utilizable — los modelos locales incluidos necesitan una GPU de hardware con al menos 6 GB

journal.cli.check.gpu.cuda.integrity = Fallo de integridad del entorno de ejecución de CUDA

journal.cli.check.gpu.low.linux = La GPU { $gpu } tiene { $gb } GB — los modelos locales incluidos necesitan al menos 6 GB

journal.cli.check.gpu.low.windows = La GPU { $gpu } tiene { $gb } GB; los modelos locales incluidos necesitan al menos 6 GB

journal.cli.check.gpu.nvidia = GPU NVIDIA con { $gb } GB

journal.cli.check.gpu.nvidia.cpu = GPU NVIDIA con { $gb } GB; un modelo se ejecuta en tu GPU; la transcripción se ejecuta en tu CPU en esta computadora

journal.cli.check.gpu.nvidia.low = la GPU NVIDIA tiene { $gb } GB — los modelos locales incluidos necesitan al menos 6 GB

journal.cli.check.gpu.nvidia.unknown = Se detectó una GPU NVIDIA, pero no se pudo leer su memoria — la preparación de la GPU es desconocida

journal.cli.check.gpu.readiness.unknown = No se pudo determinar la preparación de la GPU: { $error }

journal.cli.check.gpu.render.permission = existe un nodo render de la GPU en { $path }, pero este usuario no puede abrirlo — agrégate al grupo render con `{ $repair }`, luego cierra sesión, vuelve a entrar y ejecuta `{ $command }` de nuevo

journal.cli.check.gpu.unified = GPU NVIDIA con { $gb } GB (memoria unificada)

journal.cli.check.gpu.unified.cpu = GPU NVIDIA con { $gb } GB (memoria unificada); un modelo se ejecuta en tu GPU; la transcripción se ejecuta en tu CPU en esta computadora

journal.cli.check.gpu.unknown = no se pudo leer la memoria de la GPU. el razonamiento local necesita al menos 6 GB

journal.cli.check.gpu.vulkan = GPU Vulkan { $gpu } con { $gb } GB

journal.cli.check.gpu.vulkan.cpu = GPU Vulkan { $gpu } con { $gb } GB; un modelo se ejecuta en tu GPU; la transcripción se ejecuta en tu CPU en esta computadora

journal.cli.check.gpu.vulkan.incomplete = no se encontró una GPU NVIDIA y el sondeo de Vulkan no terminó — la preparación de la GPU es desconocida

journal.cli.check.label.ced = ced

journal.cli.check.label.disk = disco

journal.cli.check.label.gpu = gpu

journal.cli.check.label.memory = memoria

journal.cli.check.label.platform = plataforma

journal.cli.check.label.ram = ram

journal.cli.check.label.rfdetr = rfdetr

journal.cli.check.marker.blocked = [bloqueado]

journal.cli.check.marker.ok = [ok]

journal.cli.check.marker.unknown = [desconocido]

journal.cli.check.marker.warn = [aviso]

journal.cli.check.memory.available = hay suficiente memoria en total, pero ahora solo hay { $gb } GB libres — cierra algunas apps antes de que se carguen los modelos locales

journal.cli.check.memory.eligible = { $gb } GB de memoria cumple el mínimo de 16 GB

journal.cli.check.memory.minimum = Apple Silicon necesita al menos 16 GB de memoria para los modelos locales incluidos (esta Mac tiene { $gb } GB)

journal.cli.check.memory.unknown = no se pudo verificar la memoria disponible

journal.cli.check.overall.blocked = No está listo — esta computadora todavía no puede ejecutar los modelos locales incluidos.

journal.cli.check.overall.ready = Listo — instala el diario a continuación:  mira { $file }

journal.cli.check.overall.warning = Casi listo (mira los avisos de arriba) — puedes instalar el diario:  mira { $file }

journal.cli.check.platform.apple = Apple Silicon en macOS (arm64)

journal.cli.check.platform.intel = Las Mac con Intel no son compatibles — usa una Mac con Apple Silicon, o ejecuta el diario en Linux compatible.

journal.cli.check.platform.linux = Linux ({ $arch })

journal.cli.check.platform.unsupported = { $os }/{ $arch } todavía no puede ejecutar los modelos locales incluidos. las plataformas compatibles son apple silicon macos, x86_64/aarch64 linux y x86_64/aarch64 windows.

journal.cli.check.platform.windows = Windows ({ $arch })

journal.cli.check.ram.low = { $gb } GB de memoria del sistema es poco — se recomiendan 8 GB o más

journal.cli.check.ram.total = { $gb } GB de memoria del sistema

journal.cli.check.ram.unknown = no se pudo verificar la memoria del sistema

journal.cli.check.title = { $command } — ¿esta computadora puede ejecutar el diario con los modelos locales incluidos?

journal.cli.doctor.backlog.band = { $recording_count ->
    [one] banda { $band }: { $recordings } grabación lista / { $recordings_left } restantes, { $outputs } { $output_count ->
        [one] salida diaria lista
        [many] salidas diarias listas
       *[other] salidas diarias listas
    } / { $outputs_left } restantes, { $activities } { $activity_count ->
        [one] actividad restante
        [many] actividades restantes
       *[other] actividades restantes
    }, { $requests } { $request_count ->
        [one] solicitud del propietario
        [many] solicitudes del propietario
       *[other] solicitudes del propietario
    }, { $permission } en espera de permiso, { $prerequisites } en espera de prerrequisitos
    [many] banda { $band }: { $recordings } grabaciones listas / { $recordings_left } restantes, { $outputs } { $output_count ->
        [one] salida diaria lista
        [many] salidas diarias listas
       *[other] salidas diarias listas
    } / { $outputs_left } restantes, { $activities } { $activity_count ->
        [one] actividad restante
        [many] actividades restantes
       *[other] actividades restantes
    }, { $requests } { $request_count ->
        [one] solicitud del propietario
        [many] solicitudes del propietario
       *[other] solicitudes del propietario
    }, { $permission } en espera de permiso, { $prerequisites } en espera de prerrequisitos
   *[other] banda { $band }: { $recordings } grabaciones listas / { $recordings_left } restantes, { $outputs } { $output_count ->
        [one] salida diaria lista
        [many] salidas diarias listas
       *[other] salidas diarias listas
    } / { $outputs_left } restantes, { $activities } { $activity_count ->
        [one] actividad restante
        [many] actividades restantes
       *[other] actividades restantes
    }, { $requests } { $request_count ->
        [one] solicitud del propietario
        [many] solicitudes del propietario
       *[other] solicitudes del propietario
    }, { $permission } en espera de permiso, { $prerequisites } en espera de prerrequisitos
}

journal.cli.doctor.backlog.caught = al día

journal.cli.doctor.backlog.caught.capped = { $day_count ->
    [one] al día; { $days } día completado con { $unit_count ->
        [one] una unidad diaria con tope
        [many] unidades diarias con tope
       *[other] unidades diarias con tope
    }
    [many] al día; { $days } días completados con { $unit_count ->
        [one] una unidad diaria con tope
        [many] unidades diarias con tope
       *[other] unidades diarias con tope
    }
   *[other] al día; { $days } días completados con { $unit_count ->
        [one] una unidad diaria con tope
        [many] unidades diarias con tope
       *[other] unidades diarias con tope
    }
}

journal.cli.doctor.backlog.change.recorded = { $name } registró un cambio en { $location }; resuelve el cambio registrado antes de rehacer ese día

journal.cli.doctor.backlog.change.started = { $name } puede haber iniciado un cambio, pero no confirmó que terminó en { $location }; resuelve el cambio en curso antes de reprocesar

journal.cli.doctor.backlog.checked = trabajo reciente revisado

journal.cli.doctor.backlog.checked.capped = { $day_count ->
    [one] trabajo reciente revisado; { $days } día completado con { $unit_count ->
        [one] una unidad diaria con tope
        [many] unidades diarias con tope
       *[other] unidades diarias con tope
    }
    [many] trabajo reciente revisado; { $days } días completados con { $unit_count ->
        [one] una unidad diaria con tope
        [many] unidades diarias con tope
       *[other] unidades diarias con tope
    }
   *[other] trabajo reciente revisado; { $days } días completados con { $unit_count ->
        [one] una unidad diaria con tope
        [many] unidades diarias con tope
       *[other] unidades diarias con tope
    }
}

journal.cli.doctor.backlog.deferred = { $name } se detuvo en { $location }; puede reintentar en una ejecución posterior, cuando cambie el día local

journal.cli.doctor.backlog.deferred.reason = { $name } se detuvo en { $location } con el motivo { $reason }; puede reintentar en una ejecución posterior, cuando cambie el día local

journal.cli.doctor.backlog.eligible = { $name } se detuvo en { $location }; puede reintentar en la próxima ejecución elegible

journal.cli.doctor.backlog.eligible.reason = { $name } se detuvo en { $location } con el motivo { $reason }; puede reintentar en la próxima ejecución elegible

journal.cli.doctor.backlog.empty.date = ninguna

journal.cli.doctor.backlog.exhausted = { $name } se detuvo en { $location } después de su reintento automático; ejecuta { $command }

journal.cli.doctor.backlog.exhausted.reason = { $name } se detuvo en { $location } con el motivo { $reason } después de su reintento automático; ejecuta { $command }

journal.cli.doctor.backlog.hold.retry = { $day }: { $reason } (reintento { $unit })

journal.cli.doctor.backlog.inventory.partial.current = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; apartado: { $holds }
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; apartado: { $holds }
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; apartado: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.current.clear = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta
}

journal.cli.doctor.backlog.inventory.partial.last = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; último avance conocido; apartado: { $holds }
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; último avance conocido; apartado: { $holds }
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; último avance conocido; apartado: { $holds }
}

journal.cli.doctor.backlog.inventory.partial.last.clear = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; último avance conocido
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; último avance conocido
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección incompleta; último avance conocido
}

journal.cli.doctor.backlog.inventory.whole.current = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; apartado: { $holds }
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; apartado: { $holds }
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; apartado: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.current.clear = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario
}

journal.cli.doctor.backlog.inventory.whole.last = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; último avance conocido; apartado: { $holds }
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; último avance conocido; apartado: { $holds }
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; último avance conocido; apartado: { $holds }
}

journal.cli.doctor.backlog.inventory.whole.last.clear = { $quantity ->
    [one] se inspeccionó { $inspected } de { $total } día, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; último avance conocido
    [many] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; último avance conocido
   *[other] se inspeccionó { $inspected } de { $total } días, de { $oldest } a { $newest }, al { $as_of }; { $bands }; inspección de todo el diario; último avance conocido
}

journal.cli.doctor.backlog.pending = pendiente: { $days }

journal.cli.doctor.backlog.pending.counts = { $pending_count ->
    [one] { $pending } día pendiente, { $stuck } { $stuck_count ->
        [one] día atascado
        [many] días atascados
       *[other] días atascados
    }
    [many] { $pending } días pendientes, { $stuck } { $stuck_count ->
        [one] día atascado
        [many] días atascados
       *[other] días atascados
    }
   *[other] { $pending } días pendientes, { $stuck } { $stuck_count ->
        [one] día atascado
        [many] días atascados
       *[other] días atascados
    }
}

journal.cli.doctor.backlog.pending.oldest = { $pending_count ->
    [one] { $pending } día pendiente, { $stuck } { $stuck_count ->
        [one] día atascado
        [many] días atascados
       *[other] días atascados
    }; el más antiguo pendiente { $day }
    [many] { $pending } días pendientes, { $stuck } { $stuck_count ->
        [one] día atascado
        [many] días atascados
       *[other] días atascados
    }; el más antiguo pendiente { $day }
   *[other] { $pending } días pendientes, { $stuck } { $stuck_count ->
        [one] día atascado
        [many] días atascados
       *[other] días atascados
    }; el más antiguo pendiente { $day }
}

journal.cli.doctor.backlog.resolve = { $name } se detuvo en { $location }; resuelve el conflicto antes de reprocesar un día pasado

journal.cli.doctor.backlog.resolve.reason = { $name } se detuvo en { $location } con el motivo { $reason }; resuelve el conflicto antes de reprocesar un día pasado

journal.cli.doctor.backlog.retry = { $name } se detuvo en { $location }; ejecuta { $command }

journal.cli.doctor.backlog.retry.reason = { $name } se detuvo en { $location } con el motivo { $reason }; ejecuta { $command }

journal.cli.doctor.backlog.review.started = { $name } puede haber iniciado un cambio de revisión de entidades, pero no confirmó que terminó en { $location }; resuelve el cambio en curso antes de reprocesar

journal.cli.doctor.backlog.stuck = atascado: { $days }

journal.cli.doctor.backlog.unfinished = { $activity_count ->
    [one] { $activities } actividad en { $days } { $day_count ->
        [one] día completado
        [many] días completados
       *[other] días completados
    } no pudo terminar el procesamiento
    [many] { $activities } actividades en { $days } { $day_count ->
        [one] día completado
        [many] días completados
       *[other] días completados
    } no pudieron terminar el procesamiento
   *[other] { $activities } actividades en { $days } { $day_count ->
        [one] día completado
        [many] días completados
       *[other] días completados
    } no pudieron terminar el procesamiento
}

journal.cli.doctor.backlog.unknown = desconocido: { $days }

journal.cli.doctor.backlog.unknown.count = { $quantity ->
    [one] no se pudo determinar del todo; { $days } día desconocido
    [many] no se pudo determinar del todo; { $days } días desconocidos
   *[other] no se pudo determinar del todo; { $days } días desconocidos
}

journal.cli.doctor.backup.association = { $association ->
    [unbound] asociación de la configuración inicial desconocida; solo el historial
    [unknown] asociación de la configuración inicial actual no disponible
    [superseded] configuración inicial reemplazada; solo el historial
    [current] coincide con la configuración inicial elegida
   *[invalid] asociación de la configuración inicial registrada no disponible
}

journal.cli.doctor.backup.attempt.incomplete = { $outcome ->
    [success] copia de seguridad activada; último intento registrado: copia exitosa el { $date }; algunos archivos no se copiaron; { $history }; { $association }
    [incomplete] copia de seguridad activada; último intento registrado: copia incompleta el { $date }; algunos archivos no se copiaron; { $history }; { $association }
   *[failed] copia de seguridad activada; último intento registrado: falló el { $date }; algunos archivos no se copiaron; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.locked = { $outcome ->
    [success] copia de seguridad activada; último intento registrado: copia exitosa el { $date }; el repositorio estaba bloqueado; { $history }; { $association }
    [incomplete] copia de seguridad activada; último intento registrado: copia incompleta el { $date }; el repositorio estaba bloqueado; { $history }; { $association }
   *[failed] copia de seguridad activada; último intento registrado: falló el { $date }; el repositorio estaba bloqueado; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.ordinary = { $outcome ->
    [success] copia de seguridad activada; último intento registrado: copia exitosa el { $date }; { $history }; { $association }
    [incomplete] copia de seguridad activada; último intento registrado: copia incompleta el { $date }; { $history }; { $association }
   *[failed] copia de seguridad activada; último intento registrado: falló el { $date }; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.refused = { $outcome ->
    [success] copia de seguridad activada; último intento registrado: copia exitosa el { $date }; se rechazó la clave de la copia de seguridad; { $history }; { $association }
    [incomplete] copia de seguridad activada; último intento registrado: copia incompleta el { $date }; se rechazó la clave de la copia de seguridad; { $history }; { $association }
   *[failed] copia de seguridad activada; último intento registrado: falló el { $date }; se rechazó la clave de la copia de seguridad; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.timeout = { $outcome ->
    [success] copia de seguridad activada; último intento registrado: copia exitosa el { $date }; se agotó el tiempo; { $history }; { $association }
    [incomplete] copia de seguridad activada; último intento registrado: copia incompleta el { $date }; se agotó el tiempo; { $history }; { $association }
   *[failed] copia de seguridad activada; último intento registrado: falló el { $date }; se agotó el tiempo; { $history }; { $association }
}

journal.cli.doctor.backup.attempt.unknown = { $outcome ->
    [success] copia de seguridad activada; último intento registrado: copia exitosa el { $date }; causa desconocida; { $history }; { $association }
    [incomplete] copia de seguridad activada; último intento registrado: copia incompleta el { $date }; causa desconocida; { $history }; { $association }
   *[failed] copia de seguridad activada; último intento registrado: falló el { $date }; causa desconocida; { $history }; { $association }
}

journal.cli.doctor.backup.date.unknown = fecha desconocida

journal.cli.doctor.backup.details = abre la copia de seguridad o el estado en tu diario para ver los detalles

journal.cli.doctor.backup.history.incomplete.dated = { $quantity ->
    [one] última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad { $seconds } segundo; copia incompleta, algunos archivos no se copiaron; se desconocen la cobertura y la verificación actuales
    [many] última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad { $seconds } segundos; copia incompleta, algunos archivos no se copiaron; se desconocen la cobertura y la verificación actuales
   *[other] última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad { $seconds } segundos; copia incompleta, algunos archivos no se copiaron; se desconocen la cobertura y la verificación actuales
}

journal.cli.doctor.backup.history.incomplete.undated = última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad desconocida; copia incompleta, algunos archivos no se copiaron; se desconocen la cobertura y la verificación actuales

journal.cli.doctor.backup.history.success.dated = { $quantity ->
    [one] última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad { $seconds } segundo; se desconocen la cobertura y la verificación actuales
    [many] última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad { $seconds } segundos; se desconocen la cobertura y la verificación actuales
   *[other] última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad { $seconds } segundos; se desconocen la cobertura y la verificación actuales
}

journal.cli.doctor.backup.history.success.undated = última copia exitosa (histórica): instantánea { $snapshot } el { $date }, antigüedad desconocida; se desconocen la cobertura y la verificación actuales

journal.cli.doctor.backup.history.unknown = se desconoce la última copia exitosa

journal.cli.doctor.backup.invalid.history = copia de seguridad activada; el estado registrado no está disponible; { $history }

journal.cli.doctor.backup.paused.history = pusiste la copia de seguridad en pausa desde { $date }; { $history }

journal.cli.doctor.backup.snapshot.unknown = desconocida

journal.cli.doctor.backup.status.invalid.pause = el estado de la copia de seguridad no está disponible: configuración de pausa no válida

journal.cli.doctor.backup.status.invalid.settings = el estado de la copia de seguridad no está disponible: configuración de la copia de seguridad no válida

journal.cli.doctor.backup.status.invalid.switch = el estado de la copia de seguridad no está disponible: configuración de activado/desactivado no válida

journal.cli.doctor.backup.status.no.attempt = copia de seguridad activada; no hay un intento registrado

journal.cli.doctor.backup.status.off = copia de seguridad desactivada

journal.cli.doctor.backup.status.unavailable.journal = el estado de la copia de seguridad no está disponible: el diario no se puede leer

journal.cli.doctor.backup.status.unavailable.settings = el estado de la copia de seguridad no está disponible: la configuración no se puede leer

journal.cli.doctor.backup.status.unconfigured = copia de seguridad sin configurar

journal.cli.doctor.brain.age.unknown = desconocida

journal.cli.doctor.brain.no.component = ninguno

journal.cli.doctor.brain.report = { $headline }; { $state_key }={ $state }; { $reason_key }={ $reason }; { $component_key }={ $component }; { $age_key }={ $age }

journal.cli.doctor.brain.unknown = desconocido: el estado del modelo no está disponible

journal.cli.doctor.brain.unknown.reason = desconocido: { $reason }

journal.cli.doctor.capture.health.active = { $rollup }; la app de { $brand } en cada dispositivo que ha agregado a tu diario está al día

journal.cli.doctor.capture.health.data.unavailable = { $rollup }; la actividad del dispositivo no está disponible

journal.cli.doctor.capture.health.inspect = abre { $health_path } para revisar cada dispositivo

journal.cli.doctor.capture.health.no.senders = { $rollup }; la app de { $brand } todavía no agregó nada a tu diario

journal.cli.doctor.capture.health.quiet = { $rollup }; no hay subidas rechazadas registradas; mira los dispositivos para ver las últimas entregas

journal.cli.doctor.capture.health.registry.unavailable = { $rollup }; la lista de dispositivos no está disponible

journal.cli.doctor.capture.recent = la app de { $brand } en { $client } agregó por última vez hace { $hours } h

journal.cli.doctor.capture.recent.sources = { $quantity ->
    [one] la app de { $brand } en { $client } agregó por última vez hace { $hours } h ({ $sources } necesita atención)
    [many] la app de { $brand } en { $client } agregó por última vez hace { $hours } h ({ $sources } necesitan atención)
   *[other] la app de { $brand } en { $client } agregó por última vez hace { $hours } h ({ $sources } necesitan atención)
}

journal.cli.doctor.capture.rollup = { $rollup }; { $devices }

journal.cli.doctor.capture.sources = la app de { $brand } en { $client } tiene problemas para agregar { $sources }

journal.cli.doctor.capture.trouble = la app de { $brand } en { $client } tiene problemas para agregar

journal.cli.doctor.capture.trouble.sources = { $quantity ->
    [one] la app de { $brand } en { $client } tiene problemas para agregar ({ $sources } necesita atención)
    [many] la app de { $brand } en { $client } tiene problemas para agregar ({ $sources } necesitan atención)
   *[other] la app de { $brand } en { $client } tiene problemas para agregar ({ $sources } necesitan atención)
}

journal.cli.doctor.check.deferred = conjunto de revisiones diferidas: { $set }

journal.cli.doctor.client.binding.active = registros de clientes activos={ $records }; sin vincular=0; la vinculación de { $certificate_term } es la autorizada

journal.cli.doctor.client.delivery.stall.activity.unavailable = la actividad del dispositivo no está disponible

journal.cli.doctor.client.delivery.stall.inspect = abre { $health_path } para revisar la subida rechazada

journal.cli.doctor.client.delivery.stall.list.unavailable = la lista de dispositivos no está disponible

journal.cli.doctor.client.delivery.stall.no.senders = la app de { $brand } todavía no agregó nada a tu diario

journal.cli.doctor.client.delivery.stall.quiet = no hay subidas rechazadas registradas; mira los dispositivos para ver las últimas entregas

journal.cli.doctor.client.ingest.health.healthy = no hay dispositivos con la recepción fallida

journal.cli.doctor.client.ingest.health.rejection = el dispositivo { $device } tiene la recepción fallida: { $reason }, { $attempts }x desde { $since }

journal.cli.doctor.client.ingest.health.repair = actualiza o reinicia el dispositivo, y luego confirma que una subida válida quita el rechazo

journal.cli.doctor.client.transport.refusal.healthy = ningún dispositivo tuvo solicitudes rechazadas en los últimos 7 días

journal.cli.doctor.client.transport.refusal.repair = esta conexión está ocupada, no se perdió; repórtalo si sigue pasando

journal.cli.doctor.clients.ledger.unavailable = el registro de clientes autorizados no está disponible: { $reason }

journal.cli.doctor.config.dir.readable.config.unwritable = el directorio de configuración del servicio no se puede escribir: { $path }

journal.cli.doctor.config.dir.readable.config.will.create = el directorio personal se puede escribir; la instalación creará { $path }

journal.cli.doctor.config.dir.readable.config.writable = el directorio personal y el directorio de configuración del servicio se pueden escribir ({ $path })

journal.cli.doctor.config.dir.readable.home.missing = el directorio personal no existe: { $path }

journal.cli.doctor.config.dir.readable.home.unusable = el directorio personal no se puede leer ni escribir: { $path }

journal.cli.doctor.config.dir.readable.owner.unavailable = no se pudo ubicar la base del propietario: { $error }

journal.cli.doctor.config.dir.readable.owner.windows = la base del propietario de Windows solo se puede leer en Windows

journal.cli.doctor.config.restore = repara o restaura { $config_file } desde una copia de seguridad

journal.cli.doctor.context.failed = doctor no pudo resolver el contexto: { $error }

journal.cli.doctor.default.stt.ready.backend = el backend configurado es { $backend }; la preparación de parakeet no aplica

journal.cli.doctor.default.stt.ready.download = el modelo parakeet de { $coreml } no está descargado — descárgalo con: { $models_command }

journal.cli.doctor.default.stt.ready.other.model.ready = el modelo parakeet está listo en { $path }

journal.cli.doctor.default.stt.ready.unsupported = parakeet no es compatible con esta plataforma

journal.cli.doctor.delivery.rejected = el diario rechazó una subida de { $client }

journal.cli.doctor.delivery.rejected.sources = { $quantity ->
    [one] el diario rechazó una subida de { $client } ({ $sources } necesita atención)
    [many] el diario rechazó una subida de { $client } ({ $sources } necesitan atención)
   *[other] el diario rechazó una subida de { $client } ({ $sources } necesitan atención)
}

journal.cli.doctor.device.day.listing.chronicle.error = los flujos del dispositivo no están disponibles: la crónica no se puede leer: { $error }

journal.cli.doctor.device.day.listing.continuity.error = los flujos del dispositivo no están disponibles: la continuidad del flujo no se puede leer: { $error }

journal.cli.doctor.device.day.listing.continuity.malformed = los flujos del dispositivo no están disponibles: la continuidad del flujo no se puede leer: el registro de continuidad está mal formado o no es compatible

journal.cli.doctor.device.day.listing.fault = día { $day } flujo { $stream } dispositivo { $device }: { $reason }

journal.cli.doctor.device.day.listing.readable = el diario puede leer los últimos { $days } días de cada dispositivo vinculado

journal.cli.doctor.device.day.listing.record.line = los flujos del dispositivo no están disponibles: el registro de flujos no se puede leer: registro de flujo mal formado { $record }: datos mal formados en { $path } en la línea { $line }

journal.cli.doctor.device.day.listing.record.malformed = los flujos del dispositivo no están disponibles: el registro de flujos no se puede leer: registro de flujo mal formado { $record }: datos mal formados en { $path }

journal.cli.doctor.device.day.listing.registry.error = los flujos del dispositivo no están disponibles: el registro de flujos no se puede leer: { $error }

journal.cli.doctor.device.day.listing.repair = a un dispositivo que sincroniza ese día se le dice que el diario no puede leerlo; una app de { $brand } anterior a la corrección de 2026-09 lo muestra como sin conexión; revisa los segmentos del directorio del flujo nombrado y sus { $events_file }

journal.cli.doctor.device.day.listing.streams.unbound = no hay flujos de dispositivo vinculados

journal.cli.doctor.device.refusal.count = { $quantity ->
    [one] el dispositivo { $device } tuvo una solicitud rechazada: { $reason }, { $attempts }x hasta { $last_seen }
    [many] el dispositivo { $device } tuvo solicitudes rechazadas: { $reason }, { $attempts }x hasta { $last_seen }
   *[other] el dispositivo { $device } tuvo solicitudes rechazadas: { $reason }, { $attempts }x hasta { $last_seen }
}

journal.cli.doctor.devices.absent = no hay dispositivos registrados

journal.cli.doctor.devices.more = { $devices }{ $separator }+{ $extra } más

journal.cli.doctor.devices.unavailable = los registros de dispositivos no están disponibles: { $error }

journal.cli.doctor.disk.space.layout.unsupported = la raíz de instalación resuelta no es un árbol de instalación con la disposición esperada

journal.cli.doctor.disk.space.package.install = una instalación de paquete se reemplaza en el mismo lugar cuando se actualiza, así que no hace falta una segunda copia

journal.cli.doctor.disk.space.root.unresolved = no se pudo resolver la raíz de instalación desde el directorio bin de la instalación: { $path }

journal.cli.doctor.disk.space.size.overflow = desbordamiento del tamaño del árbol de instalación en { $path }

journal.cli.doctor.disk.space.size.unreadable = no se pudo medir el árbol de instalación en { $path }: { $error }

journal.cli.doctor.disk.space.space.available = { $free_gib } { $gib } libres en el sistema de archivos que contiene { $path } ({ $required_gib } { $gib } es una segunda copia de esta instalación más un margen)

journal.cli.doctor.disk.space.space.inspect = no se pudo revisar el espacio libre en { $path }: { $error }

journal.cli.doctor.disk.space.space.low = solo { $free_gib } { $gib } libres en el sistema de archivos que contiene { $path }; { $required_gib } { $gib } es una segunda copia de esta instalación más un margen

journal.cli.doctor.disk.space.space.unreadable = el espacio libre no se puede leer en { $path }

journal.cli.doctor.durability.action.recovered = no hace falta hacer nada: los registros recuperados se quedan junto a los originales y se listan aquí

journal.cli.doctor.durability.action.refused = el diario no se iniciará hasta que esto se resuelva; cada línea de arriba dice si se resuelve al reiniciar la computadora o si necesita ayuda

journal.cli.doctor.durability.action.unreadable = revisa las rutas bajo 'no se pudo revisar'; los registros recuperados, si los hay, se quedan junto a los originales y se listan aquí

journal.cli.doctor.durability.admission.helper = { $finding ->
    [exited] generación { $generation } admisión { $admission }: auxiliar { $identity }, finalizó
    [escalated] generación { $generation } admisión { $admission }: auxiliar { $identity }, retirado (escalado: true)
    [retired] generación { $generation } admisión { $admission }: auxiliar { $identity }, retirado
    [unacknowledged] generación { $generation } admisión { $admission }: auxiliar { $identity }, nunca confirmado
    [spawn] generación { $generation } admisión { $admission }: auxiliar { $identity }, falló al crear el proceso
    [unidentified] generación { $generation } admisión { $admission }: auxiliar { $identity }, no identificado
    [rejected] generación { $generation } admisión { $admission }: auxiliar { $identity }, rechazado y recogido
   *[reused] generación { $generation } admisión { $admission }: auxiliar { $identity }, pid reutilizado por otro usuario
}

journal.cli.doctor.durability.admission.service = { $finding ->
    [exited] generación { $generation } admisión { $admission }: servicio { $identity }, finalizó
    [escalated] generación { $generation } admisión { $admission }: servicio { $identity }, retirado (escalado: true)
    [retired] generación { $generation } admisión { $admission }: servicio { $identity }, retirado
    [unacknowledged] generación { $generation } admisión { $admission }: servicio { $identity }, nunca confirmado
    [spawn] generación { $generation } admisión { $admission }: servicio { $identity }, falló al crear el proceso
    [unidentified] generación { $generation } admisión { $admission }: servicio { $identity }, no identificado
    [rejected] generación { $generation } admisión { $admission }: servicio { $identity }, rechazado y recogido
   *[reused] generación { $generation } admisión { $admission }: servicio { $identity }, pid reutilizado por otro usuario
}

journal.cli.doctor.durability.clean = no hay una recuperación registrada que informar

journal.cli.doctor.durability.clean.outstanding = { $quantity ->
    [one] no hay una recuperación registrada que informar; la ejecución más reciente (generación { $generation }) todavía debe { $launches } inicio
    [many] no hay una recuperación registrada que informar; la ejecución más reciente (generación { $generation }) todavía debe { $launches } inicios
   *[other] no hay una recuperación registrada que informar; la ejecución más reciente (generación { $generation }) todavía debe { $launches } inicios
}

journal.cli.doctor.durability.closed = { $quantity ->
    [one] la generación { $generation } la cerró el inicio que pasó a ser la generación { $successor } (unix { $time }): { $total } admisión, { $exited } que ya finalizaron, { $retired } que el diario detuvo, { $never } que nunca se admitieron
    [many] la generación { $generation } la cerró el inicio que pasó a ser la generación { $successor } (unix { $time }): { $total } admisiones, { $exited } que ya finalizaron, { $retired } que el diario detuvo, { $never } que nunca se admitieron
   *[other] la generación { $generation } la cerró el inicio que pasó a ser la generación { $successor } (unix { $time }): { $total } admisiones, { $exited } que ya finalizaron, { $retired } que el diario detuvo, { $never } que nunca se admitieron
}

journal.cli.doctor.durability.closed.restarted = { $quantity ->
    [one] la generación { $generation } la cerró el inicio que pasó a ser la generación { $successor } (unix { $time }): { $total } admisión, { $exited } que ya finalizaron, { $retired } que el diario detuvo, { $never } que nunca se admitieron; la computadora se había reiniciado desde entonces
    [many] la generación { $generation } la cerró el inicio que pasó a ser la generación { $successor } (unix { $time }): { $total } admisiones, { $exited } que ya finalizaron, { $retired } que el diario detuvo, { $never } que nunca se admitieron; la computadora se había reiniciado desde entonces
   *[other] la generación { $generation } la cerró el inicio que pasó a ser la generación { $successor } (unix { $time }): { $total } admisiones, { $exited } que ya finalizaron, { $retired } que el diario detuvo, { $never } que nunca se admitieron; la computadora se había reiniciado desde entonces
}

journal.cli.doctor.durability.heading.aside = contabilidad apartada ({ $count }):

journal.cli.doctor.durability.heading.closed = ejecuciones cerradas por un inicio posterior ({ $count }):

journal.cli.doctor.durability.heading.refused = inicios rechazados ({ $count }):

journal.cli.doctor.durability.heading.unreadable = no se pudo revisar ({ $count }):

journal.cli.doctor.durability.heading.unresolved = ejecuciones que no se detuvieron de forma limpia ({ $count }):

journal.cli.doctor.durability.more = ... y { $count } más

journal.cli.doctor.durability.outstanding = { $quantity ->
    [one] la ejecución más reciente (generación { $generation }) todavía debe { $launches } inicio
    [many] la ejecución más reciente (generación { $generation }) todavía debe { $launches } inicios
   *[other] la ejecución más reciente (generación { $generation }) todavía debe { $launches } inicios
}

journal.cli.doctor.durability.refusal.restart = generación { $generation }: se perdió el registro de procesos que un retiro inició, así que el diario no se iniciará; se resuelve después de reiniciar la computadora

journal.cli.doctor.durability.refusal.support = generación { $generation }: se perdió el registro de procesos que un retiro inició, así que el diario no se iniciará; no se resuelve por su cuenta y necesita ayuda de soporte

journal.cli.doctor.durability.unresolved = generación { $generation }: { $reason }

journal.cli.doctor.executable.directory = no se pudo determinar el directorio del ejecutable: { $path }

journal.cli.doctor.execution.error = falló la ejecución de la revisión: { $kind }

journal.cli.doctor.execution.error.detail = falló la ejecución de la revisión: { $kind }: { $error }

journal.cli.doctor.facet.fix = { $unrouted_quantity ->
    [one] ese segmento no está en las listas de actividades; revisa las declaraciones de facetas con `{ $doctor_command }` y las ejecuciones recientes de sense con `{ $logs_command }`
    [many] esos segmentos no están en las listas de actividades; revisa las declaraciones de facetas con `{ $doctor_command }` y las ejecuciones recientes de sense con `{ $logs_command }`
   *[other] esos segmentos no están en las listas de actividades; revisa las declaraciones de facetas con `{ $doctor_command }` y las ejecuciones recientes de sense con `{ $logs_command }`
}

journal.cli.doctor.facet.no.active = no hay segmentos activos en los días más recientes

journal.cli.doctor.facet.routed = { $active_quantity ->
    [one] { $routed } de { $active } segmento activo desde { $since } archivado bajo una faceta
    [many] { $routed } de { $active } segmentos activos desde { $since } archivados bajo una faceta
   *[other] { $routed } de { $active } segmentos activos desde { $since } archivados bajo una faceta
}

journal.cli.doctor.facet.unrouted = { $active_quantity ->
    [one] { $unrouted } de { $active } segmento activo desde { $since } { $unrouted_quantity ->
        [one] no se archivó
        [many] no se archivaron
       *[other] no se archivaron
    } bajo ninguna faceta
    [many] { $unrouted } de { $active } segmentos activos desde { $since } { $unrouted_quantity ->
        [one] no se archivó
        [many] no se archivaron
       *[other] no se archivaron
    } bajo ninguna faceta
   *[other] { $unrouted } de { $active } segmentos activos desde { $since } { $unrouted_quantity ->
        [one] no se archivó
        [many] no se archivaron
       *[other] no se archivaron
    } bajo ninguna faceta
}

journal.cli.doctor.help = uso: { $syntax }{ $line_break }{ $line_break }Ejecuta los diagnósticos de { $brand }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $indent }{ $help_option }{ $help_gap }mostrar este mensaje de ayuda y salir{ $line_break }{ $indent }{ $verbose_option }{ $verbose_gap }imprimir el resultado de cada revisión{ $line_break }{ $indent }{ $json_option }{ $json_gap }emitir JSON en lugar de texto{ $line_break }{ $indent }{ $jsonl_option }{ $jsonl_gap }emitir eventos de un JSON por línea en lugar de texto{ $line_break }{ $indent }{ $port_option }{ $port_gap }puerto a sondear (predeterminado: { $port }){ $line_break }{ $indent }{ $readiness_option }{ $readiness_gap }ejecutar la batería de preparación de la configuración inicial{ $line_break }{ $line_break }Si '{ $command }' no está disponible, compila el doctor nativo desde esta copia de trabajo:{ $line_break }{ $build_command }

journal.cli.doctor.holds.partition = partición retenida { $partition }

journal.cli.doctor.holds.partition.stopped = la partición { $partition } estaba en ejecución cuando el diario se detuvo; el diario la retiene en el próximo inicio hasta que sus procesos ya no estén

journal.cli.doctor.holds.record.empty = el archivo de registro está vacío

journal.cli.doctor.holds.record.filename = nombre de archivo de partición no válido en el directorio de alcance; el diario igual lo lee como un registro de tarea

journal.cli.doctor.holds.record.invalid = no se puede analizar el registro: { $error }

journal.cli.doctor.holds.record.stray = entrada que no es un registro en el directorio de alcance

journal.cli.doctor.holds.record.unreadable = no se puede leer el registro: { $error }

journal.cli.doctor.holds.remove = quita { $path }: no retiene nada

journal.cli.doctor.holds.scope.unknown = formato de directorio de alcance no reconocido; el diario igual retiene cualquier tarea registrada ahí

journal.cli.doctor.holds.scope.unreadable = no se puede listar el directorio de alcance: { $error }

journal.cli.doctor.holds.stray = entrada suelta que no es un directorio en la ruta en curso de la cola de tareas

journal.cli.doctor.holds.unreadable = no se puede listar el directorio de retenciones en curso: { $error }

journal.cli.doctor.index.fix.checking = la búsqueda lo revisa por su cuenta y lo reconstruye si se confirma el daño; ejecuta { $command } para ver los detalles

journal.cli.doctor.index.fix.deletions = la búsqueda sigue reintentando quitar estos elementos; ejecuta { $command } para ver los detalles

journal.cli.doctor.index.fix.errors = ejecuta { $command } y revisa sus errores

journal.cli.doctor.index.fix.failed.files = la búsqueda reintenta estas actualizaciones en su revisión diaria; ejecuta { $command } para ver los detalles

journal.cli.doctor.index.fix.failed.work = la búsqueda reintenta este trabajo cada hora; ejecuta { $command } para ver los detalles

journal.cli.doctor.index.fix.full = ejecuta { $command }

journal.cli.doctor.index.fix.rebuild = la búsqueda reconstruye su índice desde tu diario cuando se confirma el daño; si persiste, ejecuta { $command }

journal.cli.doctor.index.fix.reset = ejecuta { $command }

journal.cli.doctor.index.fix.upgrade = actualiza { $brand } a la versión más reciente

journal.cli.doctor.index.fix.warnings = ejecuta { $command } y revisa sus avisos

journal.cli.doctor.journal.absent = no hay un diario local

journal.cli.doctor.journal.caught.up.backlog.unavailable = no se pudo determinar del todo; no se pudo leer la cola: { $error }

journal.cli.doctor.journal.caught.up.chronicle.entry.unavailable = no se pudo determinar del todo; no se pudo leer la entrada de la crónica: { $error }

journal.cli.doctor.journal.caught.up.chronicle.unavailable = no se pudo determinar del todo; no se pudo leer la crónica: { $error }

journal.cli.doctor.journal.caught.up.configs.unavailable = no se pudo determinar del todo; no se pudo leer las configuraciones diarias: { $error }

journal.cli.doctor.journal.caught.up.coverage.unavailable = no se pudo determinar la cobertura de unidades diarias el { $day }: { $error }

journal.cli.doctor.journal.caught.up.definitions.unavailable = no se pudo determinar del todo; no se pudo ubicar las definiciones del procesamiento diario: { $error }

journal.cli.doctor.journal.caught.up.facets.day.unavailable = no se pudo determinar del todo; no se pudo revisar las facetas activas del { $day }: { $error }

journal.cli.doctor.journal.caught.up.facets.unavailable = no se pudo determinar del todo; no se pudo leer las facetas: { $error }

journal.cli.doctor.journal.caught.up.inspect = abre el estado para ver el trabajo en espera y cualquier confirmación de gasto; reprocesa un día pasado para darle prioridad después de hoy

journal.cli.doctor.journal.caught.up.inventory.fix = abre el estado para ver el trabajo en espera y su siguiente paso

journal.cli.doctor.journal.caught.up.inventory.unavailable = la inspección de todo el diario no está disponible: { $error }

journal.cli.doctor.journal.caught.up.inventory.waiting = la inspección de todo el diario todavía no informó

journal.cli.doctor.journal.caught.up.outside.backlog = unidad diaria fuera de la cola reciente el { $day }

journal.cli.doctor.journal.caught.up.repair = vuelve a ejecutar { $doctor_command }; revisa los registros de estado si persiste

journal.cli.doctor.journal.caught.up.unit.identity = no se pudo leer el archivo de unidad diaria { $file_name } del día { $day }: la identidad del registro no coincide con su ruta

journal.cli.doctor.journal.caught.up.unit.invalid = no se pudo determinar del todo; archivo de unidad diaria mal formado { $file_name } del día { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.unreadable = no se pudo determinar del todo; archivo de unidad diaria ilegible { $file_name } del día { $day }: { $source }

journal.cli.doctor.journal.caught.up.unit.version = no se pudo leer el archivo de unidad diaria { $file_name } del día { $day }: versión de registro no compatible { $version }

journal.cli.doctor.journal.caught.up.units.entry.unavailable = no se pudo determinar del todo; no se pudo leer la entrada de unidad diaria del { $day }: { $error }

journal.cli.doctor.journal.caught.up.units.unavailable = no se pudo determinar del todo; no se pudo leer el directorio de unidades diarias del { $day }: { $error }

journal.cli.doctor.journal.dir.writable.move = mueve o quita { $path }, y luego vuelve a ejecutar

journal.cli.doctor.journal.dir.writable.not.directory = la ruta del diario existe, pero no es un directorio: { $path }

journal.cli.doctor.journal.dir.writable.parent.unwritable = el directorio del diario no está; el ancestro existente más cercano no se puede escribir: { $path }

journal.cli.doctor.journal.dir.writable.parent.writable = el directorio del diario no está; el directorio padre { $path } se puede escribir

journal.cli.doctor.journal.dir.writable.unwritable = el directorio del diario no se puede escribir: { $path }

journal.cli.doctor.journal.dir.writable.writable = el directorio del diario se puede escribir: { $path }

journal.cli.doctor.journal.sources.readable.healthy = el último escaneo no encontró días ilegibles

journal.cli.doctor.journal.sources.readable.inspect.folders = revisa las carpetas de días nombradas arriba — un nombre que no es una fecha de calendario no es un día que el diario pueda procesar

journal.cli.doctor.journal.sources.readable.repair.files = repara el archivo nombrado arriba y luego ejecuta { $reprocess_command } para cada día listado que ya haya terminado

journal.cli.doctor.journal.sources.readable.reprocess = ejecuta { $reprocess_command } para cada día listado que ya haya terminado

journal.cli.doctor.journal.sources.readable.stats.invalid = no se pudo revisar — las estadísticas no se pueden leer

journal.cli.doctor.journal.sources.readable.stats.missing = no se pudo revisar — no hay estadísticas para leer

journal.cli.doctor.journal.sources.readable.stats.predates = no se pudo revisar — las estadísticas son anteriores a esta versión

journal.cli.doctor.launchd.stale.plist.absent = no está el plist de launchd

journal.cli.doctor.launchd.stale.plist.arguments.missing = al plist le falta { $plist_argument }

journal.cli.doctor.launchd.stale.plist.invalid = no se pudo analizar el plist: { $plist_error }: { $error }

journal.cli.doctor.launchd.stale.plist.repair = ejecuta { $uninstall_command } y luego ejecuta { $install_command } por separado para reinstalar un servicio en segundo plano sin interfaz

journal.cli.doctor.launchd.stale.plist.target.exists = el destino del plist de launchd existe ({ $path })

journal.cli.doctor.launchd.stale.plist.target.missing = el plist apunta a un ejecutable que falta: { $path }

journal.cli.doctor.local.bin.solstone.reachable.file.invalid = { $path } no es un archivo

journal.cli.doctor.local.bin.solstone.reachable.file.missing = falta { $path }

journal.cli.doctor.local.bin.solstone.reachable.path.absent = { $brand } no está en { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.path.mismatch = { $path_variable } { $brand } se resuelve en { $resolved }, se esperaba { $expected }

journal.cli.doctor.local.bin.solstone.reachable.path.ready = { $wrapper_path } está en { $path_variable }, en { $path }

journal.cli.doctor.local.bin.solstone.reachable.path.repair = pon { $bin_dir } antes en { $path_variable }, o ejecuta { $setup_command } para volver a apuntar el envoltorio administrado

journal.cli.doctor.local.bin.solstone.reachable.symlink.ready = { $wrapper_path } enlaza simbólicamente a { $path_variable } { $brand } en { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.path.priority = pon { $install_dir } antes de { $other_dir } en tu { $path_variable }

journal.cli.doctor.local.bin.solstone.reachable.windows.ready = { $brand } está en { $path_variable }, en { $path }

journal.cli.doctor.local.bin.solstone.reachable.windows.repair = reinstala el diario para restaurar { $executable }

journal.cli.doctor.local.bin.solstone.reachable.windows.terminal = abre una ventana de terminal nueva y vuelve a intentarlo; si todavía no se encuentra { $brand }, reinstala el diario

journal.cli.doctor.local.bin.solstone.reachable.wrapper.install = ejecuta { $setup_command } para instalar el envoltorio administrado de { $brand }

journal.cli.doctor.orphan.pdf.found = { $quantity ->
    [one] { $count } PDF original en bruto sin una transcripción legible del documento: { $paths }
    [many] { $count } PDF originales en bruto sin una transcripción legible del documento: { $paths }
   *[other] { $count } PDF originales en bruto sin una transcripción legible del documento: { $paths }
}

journal.cli.doctor.orphan.segment.pdf.chronicle.unavailable = el directorio de la crónica no está disponible

journal.cli.doctor.orphan.segment.pdf.clear = todos los PDF originales en bruto tienen transcripciones legibles del documento

journal.cli.doctor.orphan.segment.pdf.repair = agrega un { $transcript_file } legible junto al PDF original y luego vuelve a ejecutar { $doctor_command }

journal.cli.doctor.package.executable.parent = el ejecutable no tiene padre

journal.cli.doctor.package.layout = no se pudieron localizar las raíces de talentos empaquetados desde el directorio del ejecutable { $directory }{ $newline }ancestro recorrido { $ancestor }{ $newline }opción de site-packages: { $site }{ $newline }opción de copia de trabajo ({ $pyproject } + { $git_dir }): { $checkout }{ $newline }anclas de share de la disposición { $bundle }, { $layout }, { $template }: { $install }{ $newline }paquete de Apple { $journal_payload }, opción: { $embedded }

journal.cli.doctor.package.missing = a la raíz de instalación { $root } le falta el directorio { $path }

journal.cli.doctor.package.none = ninguno

journal.cli.doctor.package.target = { $newline }directorio target de Cargo { $target } está fuera de cualquier copia de trabajo: un binario compilado ahí no encuentra la carga útil, mantén { $target_env } dentro de la copia de trabajo

journal.cli.doctor.package.unavailable = no se pudieron localizar las raíces de talentos empaquetados desde el directorio del ejecutable { $directory }

journal.cli.doctor.parakeet.coreml.configured = falló la revisión de parakeet: la caché instalada { $installed } no coincide con la configurada { $configured }

journal.cli.doctor.parakeet.coreml.digest = falló la revisión de parakeet: no coincide el digest del recurso { $file }: se esperaba { $expected }, se encontró { $found }

journal.cli.doctor.parakeet.coreml.failed = falló la revisión de parakeet: { $error }

journal.cli.doctor.parakeet.coreml.missing = falló la revisión de parakeet: falta el recurso: { $file }

journal.cli.doctor.parakeet.coreml.missing.error = falló la revisión de parakeet: falta el recurso: { $file }: { $error }

journal.cli.doctor.parakeet.coreml.sentinel = falló la revisión de parakeet: el centinela no está listo en { $path }

journal.cli.doctor.parakeet.coreml.size = falló la revisión de parakeet: no coincide el tamaño del recurso { $file }: se esperaba { $expected }, se encontró { $found }

journal.cli.doctor.parakeet.coreml.unreadable = falló la revisión de parakeet: no se pudo leer { $file }: { $error }

journal.cli.doctor.parakeet.cpp.executable = falló la revisión de { $parakeet_cpp }: { $name } no es ejecutable en { $path }

journal.cli.doctor.parakeet.cpp.missing = falló la revisión de { $parakeet_cpp }: falta { $name } en { $path }

journal.cli.doctor.parakeet.cpp.model = falló la revisión de { $parakeet_cpp }: falta el modelo en { $path }

journal.cli.doctor.parakeet.cpp.pin = al anclaje de { $parakeet_cpp } le falta { $key }

journal.cli.doctor.parakeet.cpp.platform = { $parakeet_cpp } no es compatible con { $os }/{ $arch }

journal.cli.doctor.parakeet.cpp.stt.ready.backend.not.selected = el backend configurado no es { $backend_name }; la revisión no aplica

journal.cli.doctor.parakeet.cpp.stt.ready.binary.unavailable = el binario de { $backend_name } no puede iniciar

journal.cli.doctor.parakeet.cpp.stt.ready.install = los artefactos de { $backend_name } no están instalados — obténlos con: { $provider_command }

journal.cli.doctor.parakeet.cpp.stt.ready.offline = { $server } no se puede conectar: tu diario no está en ejecución

journal.cli.doctor.parakeet.cpp.stt.ready.online = { $backend_name } listo (servidor y modelo coinciden con el paquete firmado, se puede conectar con el servidor)

journal.cli.doctor.parakeet.cpp.stt.ready.ready = { $backend_name } listo (binarios y modelo instalados, se puede conectar con el servidor)

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.install = instala el entorno de ejecución { $openmp } del sistema que provee { $runtime_file } y vuelve a ejecutar { $doctor_command }

journal.cli.doctor.parakeet.cpp.stt.ready.runtime.unavailable = { $backend_name } no puede iniciar: entorno de ejecución { $openmp } no disponible ({ $runtime_file })

journal.cli.doctor.parakeet.cpp.stt.ready.server.missing = { $server } no se puede conectar: tu diario está en ejecución, pero no { $server }

journal.cli.doctor.parakeet.cpp.stt.ready.start = { $server } no se puede conectar — inicia el servicio del diario: { $start_command }

journal.cli.doctor.parakeet.cpp.stt.ready.unreachable = { $server } no se puede conectar: { $error }

journal.cli.doctor.parakeet.cpp.stt.ready.unsupported = { $backend_name } solo es compatible con linux y windows

journal.cli.doctor.parakeet.phase = { $server } no se puede conectar: tu diario informa su estado como { $phase }

journal.cli.doctor.parakeet.phase.checking = { $server } no se puede conectar: tu diario informa su estado como revisando

journal.cli.doctor.parakeet.reinstall = reinstala el diario

journal.cli.doctor.parakeet.unreachable = { $cause ->
    [absent] { $server } no se puede conectar: no hay nada escuchando
    [unknown] { $server } no se puede conectar: no se puede saber si hay algo escuchando
    [runtime] { $server } no se puede conectar: no respondió nada
    [timeout] { $server } no se puede conectar: tardó demasiado en responder
   *[transport] { $server } no se puede conectar: se cortó la conexión
}

journal.cli.doctor.path.repair = corrige propiedad/permisos de { $path }

journal.cli.doctor.platform.unsupported = no es compatible con { $platform }

journal.cli.doctor.progress.check = { $seconds } s revisando { $name }

journal.cli.doctor.progress.hash = { $seconds } s verificando { $member }: { $hashed } de { $total } { $megabytes }

journal.cli.doctor.search.log.finished = no se pudo leer la última revisión de búsqueda terminada: { $error }

journal.cli.doctor.search.log.unavailable = el estado del índice de búsqueda no está disponible: { $error }

journal.cli.doctor.search.log.work = no se pudo leer la lista de trabajo de búsqueda: { $error }

journal.cli.doctor.sense.dispatch.capped.reason = al menos 99 errores de despacho de Sense desde la última sincronización correcta del manejador; último motivo: { $reason }

journal.cli.doctor.sense.dispatch.capped.unknown = al menos 99 errores de despacho de Sense desde la última sincronización correcta del manejador; el último motivo no está disponible

journal.cli.doctor.sense.dispatch.clear = no hay errores de despacho de Sense desde la última sincronización correcta del manejador

journal.cli.doctor.sense.dispatch.count.invalid = El estado de Sense está incompleto: no hay un conteo válido no negativo de errores de despacho

journal.cli.doctor.sense.dispatch.count.out.of.range = El estado de Sense está incompleto: el conteo de errores de despacho está fuera de su rango compatible

journal.cli.doctor.sense.dispatch.errors.reason = { $quantity ->
    [one] { $count } error de despacho de Sense desde la última sincronización correcta del manejador; último motivo: { $reason }
    [many] { $count } errores de despacho de Sense desde la última sincronización correcta del manejador; último motivo: { $reason }
   *[other] { $count } errores de despacho de Sense desde la última sincronización correcta del manejador; último motivo: { $reason }
}

journal.cli.doctor.sense.dispatch.errors.unknown = { $quantity ->
    [one] { $count } error de despacho de Sense desde la última sincronización correcta del manejador; el último motivo no está disponible
    [many] { $count } errores de despacho de Sense desde la última sincronización correcta del manejador; el último motivo no está disponible
   *[other] { $count } errores de despacho de Sense desde la última sincronización correcta del manejador; el último motivo no está disponible
}

journal.cli.doctor.sense.dispatch.unreachable = { $cause ->
    [absent] no se pudo confirmar el estado del despacho de Sense: no hay nada escuchando
    [unknown] no se pudo confirmar el estado del despacho de Sense: no se puede saber si hay algo escuchando
    [runtime] no se pudo confirmar el estado del despacho de Sense: no respondió nada
    [timeout] no se pudo confirmar el estado del despacho de Sense: tardó demasiado en responder
   *[transport] no se pudo confirmar el estado del despacho de Sense: se cortó la conexión
}

journal.cli.doctor.service.absent = no hay un servicio local del diario

journal.cli.doctor.service.conflict.warning = resuelve el aviso de topología del supervisor de macOS antes de cambiar el servicio del diario

journal.cli.doctor.service.crash.list = bucle de fallos: { $services }

journal.cli.doctor.service.identity.repair = ejecuta { $setup_command } para reinstalar el servicio

journal.cli.doctor.service.identity.repair.force = ejecuta { $setup_command } desde esta instalación para actualizar el servicio

journal.cli.doctor.service.identity.target.absent = la unidad del servicio está mal formada: no hay un ejecutable de destino en { $path }

journal.cli.doctor.service.identity.target.current = el destino del servicio coincide con la instalación actual: { $raw } -> { $path }

journal.cli.doctor.service.identity.target.mismatch = el destino del servicio no coincide: { $raw } se resuelve en { $resolved }, se esperaba { $expected }

journal.cli.doctor.service.identity.windows.current = el destino del servicio coincide con la instalación actual: { $command }

journal.cli.doctor.service.identity.windows.mismatch = el destino del servicio no coincide: { $mismatch }

journal.cli.doctor.service.registration.unreadable = no se pudo leer el registro del servicio del diario — { $reason }

journal.cli.doctor.service.restart.attempts = { $quantity ->
    [one] { $name } ({ $attempts } intento de reinicio)
    [many] { $name } ({ $attempts } intentos de reinicio)
   *[other] { $name } ({ $attempts } intentos de reinicio)
}

journal.cli.doctor.service.running.logs = ejecuta { $logs_command }

journal.cli.doctor.service.running.pid.invalid = el PID hijo de la sonda no cabe en un id de grupo de procesos

journal.cli.doctor.service.running.registration.no.reader = no hay lector para el registro del servicio

journal.cli.doctor.service.running.restart = ejecuta { $restart_command }; si persiste, ejecuta { $logs_command }

journal.cli.doctor.service.running.running = el servicio del diario está en ejecución

journal.cli.doctor.service.running.start = ejecuta { $start_command }

journal.cli.doctor.service.running.stopped = servicio instalado, pero no está en ejecución

journal.cli.doctor.service.running.unit.failed = la unidad del servicio del diario está en fallo

journal.cli.doctor.service.running.unreachable = { $cause ->
    [absent] no se pudo obtener un estado — no hay nada escuchando
    [unknown] no se pudo obtener un estado — no se puede saber si hay algo escuchando
    [runtime] no se pudo obtener un estado — no respondió nada
    [timeout] no se pudo obtener un estado — tardó demasiado en responder
   *[transport] no se pudo obtener un estado — se cortó la conexión
}

journal.cli.doctor.skill.state.checkout.required = el estado de skills del router solo está disponible desde una copia de trabajo del código

journal.cli.doctor.skill.state.directories.missing = los directorios de skills del router no están disponibles

journal.cli.doctor.skill.state.install = ejecuta { $skills_command }

journal.cli.doctor.skill.state.link.foreign = { $skill } apunta a otro lugar en { $path }

journal.cli.doctor.skill.state.link.missing = falta { $skill } en { $path }

journal.cli.doctor.skill.state.link.stale = enlace desactualizado de skill del router en { $path }

journal.cli.doctor.skill.state.links.ready = las skills del router { $brand } y journal están instaladas y al día

journal.cli.doctor.source.default = predeterminado

journal.cli.doctor.sources.days.more = { $days }, y { $more } más

journal.cli.doctor.sources.fold.failed = { $quantity ->
    [one] { $count } día no terminó de procesarse ({ $days })
    [many] { $count } días no terminaron de procesarse ({ $days })
   *[other] { $count } días no terminaron de procesarse ({ $days })
}

journal.cli.doctor.sources.statistics.refresh = ejecuta { $command }; revisa los registros de estado si persiste

journal.cli.doctor.sources.statistics.stale = no se pudo revisar — las estadísticas son de hace { $hours } horas

journal.cli.doctor.sources.unreadable = { $quantity ->
    [one] no se pudo examinar { $count } día ({ $days })
    [many] no se pudo examinar { $count } días ({ $days })
   *[other] no se pudo examinar { $count } días ({ $days })
}

journal.cli.doctor.sources.unreadable.cause = { $quantity ->
    [one] no se pudo examinar { $count } día ({ $days }): { $day }: { $cause }
    [many] no se pudo examinar { $count } días ({ $days }): { $day }: { $cause }
   *[other] no se pudo examinar { $count } días ({ $days }): { $day }: { $cause }
}

journal.cli.doctor.sources.unreadable.undated = { $quantity ->
    [one] no se pudo examinar { $count } día ({ $days }): ese día: { $cause }
    [many] no se pudo examinar { $count } días ({ $days }): ese día: { $cause }
   *[other] no se pudo examinar { $count } días ({ $days }): ese día: { $cause }
}

journal.cli.doctor.speakers.installation.app = la instalación de { $speakers_name } está incompleta ({ $detail }). Reparación: reinstala la app del diario.

journal.cli.doctor.speakers.installation.journal = la instalación de { $speakers_name } está incompleta ({ $detail }). Reparación: reinstala el diario.

journal.cli.doctor.speakers.ready = instalación de { $command } lista

journal.cli.doctor.speakers.reinstall.app = reinstala la app del diario

journal.cli.doctor.speakers.unavailable = instalación de { $command } no disponible

journal.cli.doctor.status.error = ERROR

journal.cli.doctor.status.fail = FAIL

journal.cli.doctor.status.ok = OK

journal.cli.doctor.status.skip = SKIP

journal.cli.doctor.status.warn = WARN

journal.cli.doctor.storage.inspection.log = privacidad del almacenamiento: { $error }

journal.cli.doctor.storage.privacy.choice = es posible que otras cuentas en esta computadora puedan leer tu diario. no se pudo hacer privada su carpeta en esta unidad. puedes dejarlo en esta unidad o moverlo a una unidad que mantiene las carpetas privadas. para dejarlo en esta unidad, elige eso en la página de estado de tu diario.

journal.cli.doctor.summary = { $total_quantity ->
    [one] doctor: { $total } revisión, { $failed } con fallo, { $warnings } { $warning_quantity ->
        [one] aviso
        [many] avisos
       *[other] avisos
    }, { $skipped } omitidas, { $errors } { $error_quantity ->
        [one] error
        [many] errores
       *[other] errores
    }
    [many] doctor: { $total } revisiones, { $failed } con fallo, { $warnings } { $warning_quantity ->
        [one] aviso
        [many] avisos
       *[other] avisos
    }, { $skipped } omitidas, { $errors } { $error_quantity ->
        [one] error
        [many] errores
       *[other] errores
    }
   *[other] doctor: { $total } revisiones, { $failed } con fallo, { $warnings } { $warning_quantity ->
        [one] aviso
        [many] avisos
       *[other] avisos
    }, { $skipped } omitidas, { $errors } { $error_quantity ->
        [one] error
        [many] errores
       *[other] errores
    }
}

journal.cli.doctor.supervisor.conflict.clear = no hay conflicto del supervisor de macOS (revisión nativa: solo examen de lanzadores ajenos; no se encontró un lanzador KeepAlive que compita)

journal.cli.doctor.supervisor.conflict.repair = { $quantity ->
    [one] quita el lanzador ajeno que apunta a { $app_path }: { $commands }; luego vuelve a ejecutar { $doctor_command }
    [many] quita los lanzadores ajenos que apuntan a { $app_path }: { $commands }; luego vuelve a ejecutar { $doctor_command }
   *[other] quita los lanzadores ajenos que apuntan a { $app_path }: { $commands }; luego vuelve a ejecutar { $doctor_command }
}

journal.cli.doctor.supervisor.conflict.report = { $quantity ->
    [one] conflicto del supervisor de macOS: { $count } lanzador { $keep_alive } ajeno apunta a { $app_path }
    [many] conflicto del supervisor de macOS: { $count } lanzadores { $keep_alive } ajenos apuntan a { $app_path }
   *[other] conflicto del supervisor de macOS: { $count } lanzadores { $keep_alive } ajenos apuntan a { $app_path }
}

journal.cli.doctor.supervisor.conflict.resolve = { $quantity ->
    [one] resuelve primero el conflicto del supervisor de macOS: quita el lanzador ajeno que apunta a { $app_path }: { $commands }; luego vuelve a ejecutar { $doctor_command }
    [many] resuelve primero el conflicto del supervisor de macOS: quita los lanzadores ajenos que apuntan a { $app_path }: { $commands }; luego vuelve a ejecutar { $doctor_command }
   *[other] resuelve primero el conflicto del supervisor de macOS: quita los lanzadores ajenos que apuntan a { $app_path }: { $commands }; luego vuelve a ejecutar { $doctor_command }
}

journal.cli.doctor.supervisor.conflict.unsupported = no es compatible con windows

journal.cli.doctor.sync.directory.required = un descendiente del directorio plano no es un directorio: { $path }

journal.cli.doctor.sync.entry.changed = la entrada cambió durante la observación: { $reason }

journal.cli.doctor.sync.entry.invalid = entrada de directorio plano no válida { $entry }: { $reason }

journal.cli.doctor.sync.entry.replaced = cambió o desapareció durante la inspección

journal.cli.doctor.sync.entry.unsafe = Instalación: necesita atención{ $newline }tu diario contiene un elemento que no se puede revisar con seguridad.{ $newline }mueve el elemento de los detalles fuera de la carpeta `{ $sync_folder }` de tu diario y vuelve a intentarlo.{ $newline }{ $newline }detalles:{ $newline }ruta: { $path }{ $newline }tipo: { $entry_type }{ $newline }motivo: { $reason }

journal.cli.doctor.sync.entry.unsafe.bytes = Instalación: necesita atención{ $newline }tu diario contiene un elemento que no se puede revisar con seguridad.{ $newline }mueve el elemento de los detalles fuera de la carpeta `{ $sync_folder }` de tu diario y vuelve a intentarlo.{ $newline }{ $newline }detalles:{ $newline }ruta: { $path }{ $newline }tipo: { $entry_type }{ $newline }motivo: { $reason }{ $newline }bytes del nombre: { $bytes }

journal.cli.doctor.sync.entry.vanished = la entrada del directorio plano desapareció al listar: { $path }

journal.cli.doctor.sync.file.required = la entrada del directorio plano no es un archivo normal: { $path }

journal.cli.doctor.sync.folder.full = Instalación: necesita atención{ $newline }tu diario tiene demasiados elementos para revisarlos con seguridad.{ $newline }revisa los elementos de la carpeta `{ $sync_folder }` de tu diario. mueve solo los que sabes que no van ahí hasta 256 o menos y vuelve a intentarlo.{ $newline }{ $newline }detalles:{ $newline }carpeta: { $folder }{ $newline }elementos encontrados: más de 256{ $newline }máximo revisado: 256

journal.cli.doctor.sync.folder.unsafe = Instalación: necesita atención{ $newline }una parte de tu diario no se puede revisar con seguridad.{ $newline }revisa la ruta indicada en los detalles. tiene que ser una carpeta común dentro de tu diario que puedas abrir antes de volver a intentarlo.{ $newline }{ $newline }detalles:{ $newline }ruta: { $path }{ $newline }motivo: { $reason }

journal.cli.doctor.sync.heartbeat = Instalación: necesita atención{ $newline }hay un latido reciente de otra ejecución.{ $newline }espera un momento y vuelve a intentarlo.

journal.cli.doctor.sync.identity.changed = cambió la identidad del directorio plano: { $path }

journal.cli.doctor.sync.io = { $operation } falló para { $path }: { $reason }

journal.cli.doctor.sync.kind.blockdevice = dispositivo de bloques

journal.cli.doctor.sync.kind.characterdevice = dispositivo de caracteres

journal.cli.doctor.sync.kind.directory = directorio

journal.cli.doctor.sync.kind.fifo = tubería con nombre

journal.cli.doctor.sync.kind.other = otro objeto del sistema de archivos

journal.cli.doctor.sync.kind.regularfile = archivo regular

journal.cli.doctor.sync.kind.socket = Socket de Unix

journal.cli.doctor.sync.kind.symlink = enlace simbólico

journal.cli.doctor.sync.local = solo esta computadora ({ $host })

journal.cli.doctor.sync.path.invalid = ruta de directorio plano no válida { $path }: { $reason }

journal.cli.doctor.sync.peer.legacy = solo esta computadora ({ $host }){ $newline }  último escritor distinto: { $writer } (máquina heredada { $prefix }...)

journal.cli.doctor.sync.peer.unknown = solo esta computadora ({ $host }){ $newline }  último escritor distinto: { $writer } (latido no identificado)

journal.cli.doctor.sync.peer.writer = solo esta computadora ({ $host }){ $newline }  último escritor distinto: { $writer } (escritor { $prefix }... ejecución { $run })

journal.cli.doctor.sync.reason.disappeared = la entrada desapareció durante la observación

journal.cli.doctor.sync.reason.nonregular = no es un archivo normal ({ $kind })

journal.cli.doctor.sync.reason.oversized = el archivo normal tiene { $size } bytes, el límite es { $limit }

journal.cli.doctor.sync.reason.read = no se pudo leer el archivo observado

journal.cli.doctor.sync.size = la entrada del directorio plano supera el límite de lectura observada: { $path } es { $kind }, { $size } bytes superan { $limit }

journal.cli.doctor.sync.symlink = un descendiente del directorio plano es un enlace simbólico: { $path }

journal.cli.doctor.sync.unsupported = Instalación: necesita atención{ $newline }volver a examinar la sincronización no está disponible en esta plataforma.

journal.cli.doctor.sync.unverifiable = Instalación: necesita atención{ $newline }no se pudo verificar el estado de arranque.{ $newline }espera un momento y vuelve a intentarlo.

journal.cli.doctor.sync.waiting = Instalación: en espera{ $newline }se encontró un latido reciente de otra ejecución.{ $newline }la app de { $brand } está en espera mientras revisa si esa actividad continúa. volverá a revisar en breve.

journal.cli.doctor.task.pace.about.cap = un trabajo lleva mucho tiempo; se detendrá automáticamente si supera su límite — no hace falta hacer nada, salvo que continúe

journal.cli.doctor.task.pace.healthy = las tareas van al ritmo

journal.cli.doctor.task.pace.status.unavailable = el estado del supervisor no está disponible

journal.cli.doctor.task.queue.holds.absent = no hay registros de partición de tareas en curso ni retenidos

journal.cli.doctor.task.queue.holds.clear = todos los registros de partición de tareas están limpios

journal.cli.doctor.tasks.duration = { $task } ({ $seconds } s de un límite de { $limit } s)

journal.cli.doctor.tasks.long = lleva mucho tiempo: { $tasks }

journal.cli.doctor.transcription.unretryable = { $quantity ->
    [one] { $count } grabación que el diario no reintentará por su cuenta ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
    [many] { $count } grabaciones que el diario no reintentará por su cuenta ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
   *[other] { $count } grabaciones que el diario no reintentará por su cuenta ({ $handler_key }: { $handler }, { $reason_key }: { $reason_code }): { $identities }
}

journal.cli.doctor.unretryable.transcribe.input.clear = 0 grabaciones que el diario no reintentará por su cuenta (handler: { $handler }, { $reason_key }: { $reason_code })

journal.cli.doctor.unretryable.transcribe.input.days.unreadable = no se pudo leer los días de la crónica: { $error }

journal.cli.doctor.unretryable.transcribe.input.directory.unreadable = no se puede leer el directorio del segmento { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.entry.unreadable = no se puede leer la entrada del segmento en { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.handler.missing = al registro de procesamiento en { $path } le falta el campo handler

journal.cli.doctor.unretryable.transcribe.input.identity.invalid = identidad de segmento no válida en { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.open = no se puede abrir { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.read = no se puede leer { $path }: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.encoding = la primera fila en { $path } no es utf-8 válido: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.json = la primera fila en { $path } no es json válido: { $error }

journal.cli.doctor.unretryable.transcribe.input.row.large = la primera fila en { $path } supera el límite de bytes

journal.cli.doctor.unretryable.transcribe.input.row.unavailable = la primera fila en { $path } no se puede establecer o supera el límite de bytes

journal.cli.doctor.unretryable.transcribe.input.scan.failed = no se pudo determinar cuántas grabaciones el diario no reintentará por su cuenta ({ $reason })

journal.cli.doctor.unretryable.transcribe.input.segments.unreadable = no se pudo listar los segmentos del día { $day }: { $error }

journal.cli.doctor.usage.argument = uso: { $syntax }{ $line_break }{ $command }: error: argumento inesperado

journal.cli.doctor.usage.formats = uso: { $syntax }{ $line_break }{ $command }: error: { $json_flag } y { $jsonl_flag } se excluyen mutuamente

journal.cli.doctor.usage.port = uso: { $syntax }{ $line_break }{ $command }: error: { $port_flag } requiere un entero

journal.cli.doctor.vad.contract = el auxiliar de { $vad } se ejecutó, pero no informó el contrato de uso con stdin cerrado (salida { $exit_code }): { $stderr }

journal.cli.doctor.vad.executable = no se pudo determinar el ejecutable actual del auxiliar de { $vad }: { $error }

journal.cli.doctor.vad.layout = la búsqueda del auxiliar de { $vad } está fuera de la carga útil de la app firmada en ejecución

journal.cli.doctor.vad.loader = no se pudo cargar { $library } para el auxiliar de { $vad }

journal.cli.doctor.vad.missing = falta el binario del auxiliar de { $vad }: { $path }

journal.cli.doctor.vad.parent = el ejecutable actual no tiene directorio padre para buscar el auxiliar de { $vad }

journal.cli.doctor.vad.probe.log = falló la sonda del auxiliar de { $vad }: { $error }

journal.cli.doctor.vad.ready = el auxiliar de { $vad } se puede iniciar (contrato de uso con stdin cerrado)

journal.cli.doctor.vad.repair.install = reinstala el diario y luego vuelve a ejecutar { $doctor_command }

journal.cli.doctor.vad.repair.install.app = reinstala la app del diario y luego vuelve a ejecutar { $doctor_command }

journal.cli.doctor.vad.repair.restart = reinicia el diario y luego vuelve a ejecutar { $doctor_command }; si sigue agotando el tiempo, reinstala el diario

journal.cli.doctor.vad.repair.restart.app = reinicia la app del diario y luego vuelve a ejecutar { $doctor_command }; si sigue agotando el tiempo, reinstala la app del diario

journal.cli.doctor.vad.spawn = no se pudo ejecutar el auxiliar de { $vad }: { $cause }

journal.cli.doctor.vad.timeout = el auxiliar de { $vad } no salió dentro de { $timeout }

journal.cli.doctor.vad.unresolved = no se pudo resolver la ruta del auxiliar de { $vad }: { $cause }

journal.cli.doctor.windows.onnx.helper = el auxiliar de { $onnx } no es el miembro declarado de la carga útil de la app firmada

journal.cli.doctor.windows.onnx.member = la carga útil de la app firmada { $onnx } rechaza { $member }: { $error }

journal.cli.doctor.windows.onnx.platform = la verificación del paquete { $onnx } de { $windows } requiere un entorno de ejecución de { $windows }

journal.cli.doctor.windows.onnx.verification = no se pudo verificar la carga útil de la app firmada { $onnx }: { $error }

journal.cli.doctor.windows.parakeet.executable = no se pudo determinar el ejecutable del diario en ejecución: { $error }

journal.cli.doctor.windows.parakeet.layout = el ejecutable del diario en ejecución no está en el directorio bin del paquete: { $path }

journal.cli.doctor.windows.parakeet.member = la carga útil de la app firmada { $parakeet_name } rechaza { $member }: { $error }

journal.cli.doctor.windows.parakeet.parent = el ejecutable del diario en ejecución no tiene un directorio contenedor: { $path }

journal.cli.doctor.windows.parakeet.platform = La verificación del paquete { $parakeet_name } de Windows requiere entorno de ejecución Windows

journal.cli.doctor.windows.parakeet.root = el directorio bin del paquete no tiene una raíz de paquete: { $path }

journal.cli.doctor.windows.parakeet.verification = no se pudo verificar la carga útil de la app firmada { $parakeet_name }: { $error }

journal.cli.host.arguments.invalid = { $command }: error: argumentos no válidos

journal.cli.host.backfill.facet.ids.help = uso: { $syntax }{ $line_break }{ $line_break }Completa los identificadores { $term } que faltan en todas las declaraciones de facetas.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_4 }Escribe en disco los id de facetas completados (por defecto es una prueba en seco){ $line_break }

journal.cli.host.backfill.facet.ids.solstone.journal.backfill.facet.ids.error = { $command_solstone_journal_backfill_facet_ids }: error: { $error }

journal.cli.host.backfill.facet.ids.usage = uso: { $syntax }{ $line_break }

journal.cli.host.backfill.facets.applied = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Se examinó { $scanned } faceta; se completó un identificador para { $backfilled } faceta.
        [many] Se examinó { $scanned } faceta; se completaron identificadores para { $backfilled } facetas.
        *[other] Se examinó { $scanned } faceta; se completaron identificadores para { $backfilled } facetas.
        }
    [many] { $backfilled_quantity ->
        [one] Se examinaron { $scanned } facetas; se completó un identificador para { $backfilled } faceta.
        [many] Se examinaron { $scanned } facetas; se completaron identificadores para { $backfilled } facetas.
        *[other] Se examinaron { $scanned } facetas; se completaron identificadores para { $backfilled } facetas.
        }
    *[other] { $backfilled_quantity ->
        [one] Se examinaron { $scanned } facetas; se completó un identificador para { $backfilled } faceta.
        [many] Se examinaron { $scanned } facetas; se completaron identificadores para { $backfilled } facetas.
        *[other] Se examinaron { $scanned } facetas; se completaron identificadores para { $backfilled } facetas.
        }
    }

journal.cli.host.backfill.facets.dry = { $scanned_quantity ->
    [one] { $backfilled_quantity ->
        [one] Prueba en seco: se examinó { $scanned } faceta; { $backfilled } faceta requiere un id (pasa { $literal_commit } para aplicar).
        [many] Prueba en seco: se examinó { $scanned } faceta; { $backfilled } facetas requieren id (pasa { $literal_commit } para aplicar).
        *[other] Prueba en seco: se examinó { $scanned } faceta; { $backfilled } facetas requieren id (pasa { $literal_commit } para aplicar).
        }
    [many] { $backfilled_quantity ->
        [one] Prueba en seco: se examinaron { $scanned } facetas; { $backfilled } faceta requiere un id (pasa { $literal_commit } para aplicar).
        [many] Prueba en seco: se examinaron { $scanned } facetas; { $backfilled } facetas requieren id (pasa { $literal_commit } para aplicar).
        *[other] Prueba en seco: se examinaron { $scanned } facetas; { $backfilled } facetas requieren id (pasa { $literal_commit } para aplicar).
        }
    *[other] { $backfilled_quantity ->
        [one] Prueba en seco: se examinaron { $scanned } facetas; { $backfilled } faceta requiere un id (pasa { $literal_commit } para aplicar).
        [many] Prueba en seco: se examinaron { $scanned } facetas; { $backfilled } facetas requieren id (pasa { $literal_commit } para aplicar).
        *[other] Prueba en seco: se examinaron { $scanned } facetas; { $backfilled } facetas requieren id (pasa { $literal_commit } para aplicar).
        }
    }

journal.cli.host.check.help = uso: { $syntax }{ $line_break }{ $line_break }Veredicto de preparación para los modelos locales empaquetados del diario.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }emite el veredicto de preparación como { $term } para los agentes{ $line_break }

journal.cli.host.check.usage = uso: { $syntax }{ $line_break }

journal.cli.host.config.admit.rewrite.identity.admission = solstone journal config: se rechazó la admisión de identidad: { $error }

journal.cli.host.config.admit.rewrite.identity.owner = solstone journal config: se rechazó el propietario de la identidad: { $error }

journal.cli.host.config.admit.rewrite.identity.root = solstone journal config: se rechazó la raíz de la identidad: { $error }

journal.cli.host.config.admit.rewrite.path = solstone journal config: se rechazó la ruta del diario: { $error }

journal.cli.host.config.corrupt.settings.file.at.path.couldn = no se pudo leer tu archivo de configuración en { $path }. no se cambió tu configuración. repara el archivo o restaura { $literal_1 } desde una copia de seguridad y vuelve a intentarlo.

journal.cli.host.config.cross.filesystem.cannot.move.across.filesystems.filesystem = solstone journal config: rechazado: no se puede mover entre sistemas de archivos (id de sistema de archivos { $source_device } y { $target_device }); detén tu diario con '{ $literal_1 }', copia la carpeta del diario a la ruta nueva, usa { $literal_3 } y vuelve a iniciarlo con '{ $literal_2 }'

journal.cli.host.config.decide.move.requires.not.active.target = solstone journal config: rechazado: { $literal_3 } requiere un destino que no esté activo; el actual es { $current_state } y el destino es { $target_state }; opciones válidas: { $literal_1 }, { $literal_2 }

journal.cli.host.config.execute.already.set.path = solstone journal config: el diario ya está establecido en { $path }

journal.cli.host.config.execute.before.mutation = solstone journal config: rechazado antes de la modificación: { $error }

journal.cli.host.config.execute.warning.force.bypasses.confirmation.and = solstone journal config: aviso: { $literal_1 } omite la confirmación y las revisiones de actividad del destino

journal.cli.host.config.existing.target.move.target.already.exists.path = solstone journal config: rechazado: el destino del traslado ya existe: { $path }

journal.cli.host.config.help = uso: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_10 }muestra la ruta configurada del diario y la fuente{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_2 }cambia la ruta del diario que usa esta instalación{ $line_break }

journal.cli.host.config.identity.root.from.current.executable = solstone journal config: { $error }

journal.cli.host.config.journal.path.contains.shell.active.character = solstone journal config: rechazado: la ruta del diario contiene un carácter activo en el shell: { $path }

journal.cli.host.config.journal.path.source.tree.fallback.path = solstone journal config: rechazado: { $path } es la ruta de respaldo del árbol de código, pero esto no es una copia de trabajo del código

journal.cli.host.config.maybe.restart.current.service.could.not.restart.service.at = no se pudo reiniciar el servicio en la carpeta original del diario

journal.cli.host.config.missing.current.move.source.does.not.exist = solstone journal config: rechazado: el origen del traslado no existe: { $path }

journal.cli.host.config.missing.parent.move.target.parent.does.not = solstone journal config: rechazado: no existe el directorio padre del destino del traslado: { $directory }

journal.cli.host.config.plan.action.action = acción:  { $action }

journal.cli.host.config.plan.current.left.intact.re.adopt = el diario actual queda intacto. para volver a adoptarlo más tarde: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.plan.current.path.state = actual: { $path } ({ $state })

journal.cli.host.config.plan.dry.run.yes.nothing.will = prueba en seco: sí; no se cambiará nada

journal.cli.host.config.plan.filesystem.different.devices = sistema de archivos: dispositivos distintos

journal.cli.host.config.plan.filesystem.same.device = sistema de archivos: el mismo dispositivo

journal.cli.host.config.plan.move = mover

journal.cli.host.config.plan.plan.summary = { $literal_1 } - resumen del plan

journal.cli.host.config.plan.proceed = proceder

journal.cli.host.config.plan.re.run.with.yes.proceed = vuelve a ejecutar con { $literal_1 } para proceder

journal.cli.host.config.plan.switch = cambiar

journal.cli.host.config.plan.target.path.state = destino:  { $path } ({ $state })

journal.cli.host.config.preflight.coordinates.saved.location.not.recognized.or = no se reconoce la ubicación guardada del diario o no se pudo leer; no se cambió la configuración

journal.cli.host.config.preflight.coordinates.settings.could.not.be.read = no se pudo leer tu configuración; no se cambió la configuración: { $error }

journal.cli.host.config.prepare.folder.relocation.requires.managed.posix = el traslado de la carpeta del diario requiere una instalación { $literal_1 } administrada

journal.cli.host.config.read.wrapper.targets.cannot.read.path.e = solstone journal config: rechazado: no se puede leer { $path }: { $e }

journal.cli.host.config.read.wrapper.targets.location.changed.while.this.command = la ubicación del diario cambió mientras este comando esperaba; vuelve a intentar el cambio de carpeta

journal.cli.host.config.refusal.current.current.state.and.target = solstone journal config: rechazado: actual es { $current_state } y destino es { $target_state }; indicadores válidos: { $flags }

journal.cli.host.config.rewrite.preflighted.with.path.not.utf8 = la ruta del diario no es { $literal_1 }

journal.cli.host.config.run.move.could.not.stop.service.before = solstone journal config: no se pudo detener el servicio antes del traslado ({ $error })

journal.cli.host.config.run.move.could.not.stop.service.before.stop.service.before.move = solstone journal config: no se pudo detener el servicio antes del traslado

journal.cli.host.config.run.move.move.failed.e = solstone journal config: no se pudo mover: { $e }

journal.cli.host.config.run.move.moved.wrapper.updated.service.restarted = el diario se movió; se actualizó el envoltorio; se reinició el servicio.

journal.cli.host.config.run.move.remains.at.target.could.not = el diario sigue en { $target }; no se pudo terminar de guardar la ubicación nueva: { $e }; reintento: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.remains.at.target.retry.path = el diario sigue en { $target }; reintento: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.move.service.installed.but.not.running = el servicio está instalado, pero no está en ejecución; el diario se movió; se actualizó el envoltorio.

journal.cli.host.config.run.move.service.not.installed.moved.wrapper = el servicio no está instalado; el diario se movió; se actualizó el envoltorio.

journal.cli.host.config.run.move.wrapper.updated.target.but.service = se actualizó el envoltorio a { $target }, pero no se pudo iniciar el servicio; reinicia manualmente

journal.cli.host.config.run.switch.cannot.create.path.e = solstone journal config: rechazado: no se puede crear { $path }: { $e }

journal.cli.host.config.run.switch.cannot.inspect.managed.wrappers = solstone journal config: rechazado: no se pueden inspeccionar los wrappers administrados: { $error }

journal.cli.host.config.run.switch.cannot.lock.managed.wrappers = solstone journal config: rechazado: no se pueden bloquear los wrappers administrados: { $error }

journal.cli.host.config.run.switch.could.not.finish.saving.new = no se pudo terminar de guardar la ubicación nueva del diario: { $message }; reintento: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.finish.saving.new.journal.path.switch.yes = no se pudo terminar de guardar la ubicación nueva del diario: { $e }; reintento: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.could.not.stop.service.before = no se pudo detener el servicio antes de cambiar la carpeta del diario

journal.cli.host.config.run.switch.now.at.target.could.not = tu diario ahora está en { $target }; no se pudo actualizar su servicio: { $error }; reintento: { $literal_1 } { $path } { $literal_2 } { $literal_3 }

journal.cli.host.config.run.switch.service.installed.but.not.running = el servicio está instalado, pero no está en ejecución; se actualizó el envoltorio.

journal.cli.host.config.run.switch.service.not.installed.wrapper.updated = el servicio no está instalado; se actualizó el envoltorio.

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service = solstone journal config: se reescribió el envoltorio a { $target }, pero no se pudo ejecutar el reinicio del servicio del diario ({ $error }); reinicia manualmente

journal.cli.host.config.run.switch.wrapper.rewritten.target.but.service.investigate.and.restart.manually = solstone journal config: se reescribió el envoltorio a { $target }, pero '{ $literal_1 }' salió con { $code }; investiga y reinicia manualmente

journal.cli.host.config.run.switch.wrapper.updated.service.restarted = se actualizó el envoltorio; se reinició el servicio.

journal.cli.host.config.run.unsupported.folder.service.action = acción de servicio de la carpeta del diario no compatible

journal.cli.host.config.service.summary.service.installed.and.running.will = servicio: instalado y en ejecución; se detendrá, hará el traslado, reescribirá el envoltorio y se reiniciará

journal.cli.host.config.service.summary.service.installed.and.running.will.will.rewrite.wrapper.restart = servicio: instalado y en ejecución; reescribirá el envoltorio y se reiniciará

journal.cli.host.config.service.summary.service.installed.but.not.running = servicio: instalado, pero no en ejecución; hará el traslado y reescribirá el envoltorio

journal.cli.host.config.service.summary.service.installed.but.not.running.running.will.rewrite.wrapper = servicio: instalado, pero no en ejecución; reescribirá el envoltorio

journal.cli.host.config.service.summary.service.not.installed.will.move = servicio: no instalado; hará el traslado y reescribirá el envoltorio

journal.cli.host.config.service.summary.service.not.installed.will.rewrite = servicio: no instalado; reescribirá el envoltorio

journal.cli.host.config.show.next.next = siguiente: { $next }

journal.cli.host.config.show.source.built.in.default = valor predeterminado integrado ({ $literal_1 })

journal.cli.host.config.show.source.source.tree.fallback = alternativa del árbol de código fuente

journal.cli.host.config.show.source.user.toml = configuración de usuario ({ $literal_1 })

journal.cli.host.config.state.active = activo

journal.cli.host.config.state.not.active = no activo

journal.cli.host.config.usage = uso: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $line_break }

journal.cli.host.config.wrapper.next.move.path.aside.then.run = mueve { $path } a un lado y luego ejecuta '{ $literal_1 }' desde el checkout del código fuente de solstone

journal.cli.host.config.wrapper.refusal.wrapper.not.managed.wrapper.run = solstone journal config: rechazado: { $wrapper } no es un envoltorio administrado (ejecuta '{ $literal_1 }' desde el checkout del código fuente de solstone para instalar el envoltorio primero)

journal.cli.host.contract.build.bundle.browser.schema.bytes.diverged.from = contrato: los bytes del esquema del navegador divergieron de { $literal_1 }

journal.cli.host.contract.build.bundle.cannot.read.browser.schema.path = contrato: no se puede leer el esquema del navegador { $path }: { $error }

journal.cli.host.contract.build.bundle.invalid.json.schema.path = contrato: { $literal_1 } Schema no válido { $path }: { $error }

journal.cli.host.contract.build.bundle.missing.required.schema.sources.sources = contrato: faltan fuentes de esquema obligatorias: { $sources }

journal.cli.host.contract.build.bundle.path.duplicate.format.id.format = contrato: { $path }: id de formato del contrato del diario duplicado '{ $format_id }'

journal.cli.host.contract.build.bundle.path.format.id.must.be = contrato: { $path }: { $literal_1 } debe ser una cadena

journal.cli.host.contract.build.bundle.schema.must.be.json.object = contrato: el esquema debe ser un objeto { $literal_1 }: { $path }

journal.cli.host.contract.build.help = uso: { $syntax }{ $line_break }{ $line_break }Crea el bundle del contrato del diario.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_15 }Revisa si el bundle está al día sin escribir.{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Checkout del contrato o raíz del paquete instalado.{ $line_break }

journal.cli.host.contract.build.usage = uso: { $syntax }{ $line_break }

journal.cli.host.contract.check.help = uso: { $syntax }{ $line_break }{ $line_break }Revisa el bundle y los archivos del diario contra sus esquemas.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }Raíz de diario adicional para validar (repetible).{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_11 }Checkout del contrato o raíz del paquete instalado.{ $line_break }

journal.cli.host.contract.check.usage = uso: { $syntax }{ $line_break }

journal.cli.host.contract.classify.breaking.changes.bundle.malformed = el bundle de solstone journal contract está malformado

journal.cli.host.contract.classify.breaking.changes.format.removed.format = { $format }: formato quitado

journal.cli.host.contract.classify.breaking.changes.format.removed.key.field.field = { $format }: campo clave quitado '{ $field }'

journal.cli.host.contract.classify.breaking.changes.format.removed.producer.path.path = { $format }: ruta de productor quitada '{ $path }'

journal.cli.host.contract.discover.schema.sources.path.missing.meta = contrato: { $path }: falta { $contract_meta }

journal.cli.host.contract.fixture.journal.fixture.missing.path = contrato: falta el diario de fixture: { $path }

journal.cli.host.contract.format.errors.invalid.json.schema = { $literal_1 } schema no válido: { $error }

journal.cli.host.contract.from.root.package.tree.not.found.path = contrato: no se encontró el árbol de paquetes de solstone: { $path }

journal.cli.host.contract.help = uso: { $syntax }{ $line_break }{ $line_break }Crea y valida el bundle del contrato del diario.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_15 }Crea el bundle del contrato.{ $line_break }{ $spaces_4 }{ $subcommand_8 }{ $spaces_15 }Revisa el bundle y los archivos del diario.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.contract.load.json.invalid.json.in.path = contrato: { $literal_1 } no válido en { $path }: { $error }

journal.cli.host.contract.metadata.path.allowed.producers.must.be = contrato: { $path }: { $literal_1 } debe ser una lista

journal.cli.host.contract.metadata.path.key.fields.must.be = contrato: { $path }: { $literal_1 } debe ser una lista

journal.cli.host.contract.metadata.path.missing.metadata.key = contrato: { $path }: falta el metadato del contrato: { $key }

journal.cli.host.contract.resolve.could.not.inspect.current.executable = contrato: no se pudo inspeccionar el ejecutable actual: { $error }

journal.cli.host.contract.resolve.from.executable.could.not.locate.installed.package = contrato: no se pudo localizar el paquete de solstone instalado ni el checkout del código fuente desde { $path }

journal.cli.host.contract.run.build.cannot.create.bundle.directory = contrato: no se puede crear el directorio del bundle: { $error }

journal.cli.host.contract.run.build.cannot.write.path = contrato: no se puede escribir { $path }: { $error }

journal.cli.host.contract.run.build.path.stale.run.build = { $path } está desactualizado; ejecuta `{ $literal_1 }`

journal.cli.host.contract.run.build.wrote.path = se escribió { $path }

journal.cli.host.contract.run.check.bundle.artifact.missing.path = contrato: falta el artefacto del bundle: { $path }

journal.cli.host.contract.run.check.no.covered.files.found.under = contrato: no se encontraron archivos cubiertos por el contrato en { $path }

journal.cli.host.contract.run.check.tree.not.found.path.expected = contrato: no se encontró el árbol del diario: { $path } (se esperaba { $expected })

journal.cli.host.contract.usage = uso: { $syntax }{ $line_break }

journal.cli.host.contract.validate.contract.file.unsupported.file.kind.kind = tipo de archivo de contrato no compatible: { $kind }

journal.cli.host.contract.validate.headered.jsonl.headered.jsonl.requires.header.line = { $literal_1 } requiere una línea de encabezado

journal.cli.host.contract.validate.headered.jsonl.invalid.utf8 = { $literal_1 } no válido: { $error }

journal.cli.host.contract.validate.headered.jsonl.line.must.be.json.object = la línea debe ser un objeto { $literal_1 }

journal.cli.host.contract.validate.headered.jsonl.missing.schema.definition = falta la definición del esquema

journal.cli.host.contract.validate.json.invalid.json = { $literal_1 } no válido: { $error }

journal.cli.host.contract.validated = { $quantity ->
    [one] se validó { $display_count } archivo cubierto por el contrato en { $root }
    [many] se validaron { $display_count } archivos cubiertos por el contrato en { $root }
   *[other] se validaron { $display_count } archivos cubiertos por el contrato en { $root }
    }

journal.cli.host.contract.walk.four.levels.cannot.read.entry = contrato: no se puede leer la entrada del diario: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.path = contrato: no se puede leer { $path }: { $error }

journal.cli.host.contract.walk.schema.files.cannot.read.schema.entry = contrato: no se puede leer la entrada del esquema: { $error }

journal.cli.host.convey.help = uso: { $syntax }{ $line_break }{ $line_break }interfaz web de { $term }{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Puerto en el que se sirve{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_2 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_4 }Activa el registro de depuración{ $line_break }

journal.cli.host.convey.usage = uso: { $syntax }{ $line_break }

journal.cli.host.cortex.help = uso: { $syntax }{ $line_break }{ $line_break }{ $term } { $term_4 } Gestor de talentos{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_2 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_4 }Activa el registro de depuración{ $line_break }

journal.cli.host.cortex.service.cortex.service.failed = falló el servicio de cortex: { $error }

journal.cli.host.cortex.usage = uso: { $syntax }{ $line_break }

journal.cli.host.down.help = uso: { $syntax }{ $line_break }{ $line_break }Detén el servicio del diario.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.facet.candidates.help = uso: { $syntax }{ $line_break }{ $line_break }Registra candidatos recurrentes para la revisión de facetas.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Activa el registro de depuración{ $line_break }

journal.cli.host.facet.candidates.recorded = { $quantity ->
    [one] Se registró o actualizó { $display_count } candidato de faceta.
    [many] Se registraron o actualizaron { $display_count } candidatos de faceta.
   *[other] Se registraron o actualizaron { $display_count } candidatos de faceta.
    }

journal.cli.host.facet.candidates.run.facet.candidates = solstone journal facet-candidates: error: { $error }

journal.cli.host.facet.candidates.run.facet.candidates.root.path.not = solstone journal facet-candidates: error: la raíz del diario { $path } no es un directorio

journal.cli.host.facet.candidates.usage = uso: { $syntax }{ $line_break }

journal.cli.host.grab.help = uso: { $syntax }{ $line_break }{ $line_break }Recorre los fotogramas de pantalla observados y, de forma opcional, escribe las imágenes de los fotogramas.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_11 }Tokens de ruta: { $path_tokens }{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_6 }Escribe aquí la imagen del fotograma seleccionado ({ $extension }, { $extension_12 }, { $extension_13 } o{ $line_break }{ $spaces_17 }{ $extension_15 }).{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_8 }Reemplaza una ruta de salida existente.{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_9 }Emite { $term } en lugar de una tabla o de texto simple.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_2 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_4 }Activa el registro de depuración{ $line_break }

journal.cli.host.grab.usage = uso: { $syntax }{ $line_break }

journal.cli.host.health.callosum.clients = { $quantity ->
    [one] Callosum: { $clients } cliente
    [many] Callosum: { $clients } clientes
   *[other] Callosum: { $clients } clientes
    }

journal.cli.host.health.ced.ready = el motor de etiquetas de sonido de ced.cpp y el modelo están listos

journal.cli.host.health.ced.unsupported = instalación de ced: plataforma no compatible { $os }/{ $arch }; se omiten los recursos de etiquetas de sonido de ced

journal.cli.host.health.crash.attempts = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } intento de reinicio
    [many] { $space_2 }{ $name } { $attempts } intentos de reinicio
   *[other] { $space_2 }{ $name } { $attempts } intentos de reinicio
    }

journal.cli.host.health.crash.reason = { $quantity ->
    [one] { $space_2 }{ $name } { $attempts } intento de reinicio{ $space_2 }{ $reason }
    [many] { $space_2 }{ $name } { $attempts } intentos de reinicio{ $space_2 }{ $reason }
   *[other] { $space_2 }{ $name } { $attempts } intentos de reinicio{ $space_2 }{ $reason }
    }

journal.cli.host.health.crashed = { $line_break }Cierre inesperado:{ $line_break }

journal.cli.host.health.heartbeat.ok = Latido: ok{ $line_break }

journal.cli.host.health.heartbeat.stale = { $line_break }Latido: DESACTUALIZADO ({ $stale })

journal.cli.host.health.held = { $space_2 }retenido { $partition } { $reason }

journal.cli.host.health.help = uso: { $syntax }{ $line_break }{ $line_break }Muestra el estado de servicio del supervisor que se conserva.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }activa la salida de depuración{ $line_break }

journal.cli.host.health.logs.help = uso: { $syntax }{ $line_break }{ $line_break }Ve los registros operativos del servicio.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_18 }cantidad de líneas a mostrar (predeterminado: 5){ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_20 }sigue la salida del registro{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }muestra filas en { $argument } o después{ $line_break }{ $spaces_2 }{ $option_13 }{ $spaces_8 }muestra filas de un servicio{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }muestra filas que coinciden con { $argument_16 }{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }activa la salida de depuración{ $line_break }

journal.cli.host.health.logs.usage = uso: { $syntax }{ $line_break }

journal.cli.host.health.media.absent = Procesador de contenido multimedia: no informa

journal.cli.host.health.media.depth.stale = Procesador de contenido multimedia: { $depth } en espera (desactualizado, { $seconds } s desde la baliza)

journal.cli.host.health.media.idle = Procesador de contenido multimedia: inactivo

journal.cli.host.health.media.pending = Procesador de contenido multimedia: { $depth } en espera

journal.cli.host.health.media.stale = Procesador de contenido multimedia: desactualizado ({ $seconds } s desde la baliza)

journal.cli.host.health.not.running = No se puede conectar: tu diario no está en ejecución{ $line_break }

journal.cli.host.health.queue = { $space_2 }en cola { $name } { $count }

journal.cli.host.health.queue.hold = { $space_2 }retención de cola { $reason } { $details }

journal.cli.host.health.rfdetr.assets = La detección de objetos está degradada porque sus recursos de RF-DETR no están disponibles. Las descripciones de pantalla continuarán. Usa `{ $command }` para revisar o reparar los recursos de RF-DETR.

journal.cli.host.health.rfdetr.package = la detección de objetos no está disponible. repara o reinstala tu diario.

journal.cli.host.health.rfdetr.ready = el motor de detección de objetos de rf-detr.cpp y el modelo están listos

journal.cli.host.health.rfdetr.unsupported = instalación de rf-detr: plataforma no compatible { $os }/{ $arch }; se omiten los recursos de detección de objetos de rf-detr

journal.cli.host.health.runtime = No se puede conectar: el entorno de ejecución del estado no está disponible: { $message }{ $line_break }

journal.cli.host.health.service.days = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $days } d

journal.cli.host.health.service.days.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $days } d { $hours } h

journal.cli.host.health.service.days.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $days } d { $hours } h { $minutes } min

journal.cli.host.health.service.days.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $days } d { $minutes } min

journal.cli.host.health.service.hours = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $hours } h

journal.cli.host.health.service.hours.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $hours } h { $minutes } min

journal.cli.host.health.service.minutes = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $minutes } min

journal.cli.host.health.service.seconds = { $space_2 }{ $name } pid { $pid }{ $space_2 }tiempo activo { $seconds } s

journal.cli.host.health.services = Servicios:{ $line_break }

journal.cli.host.health.socket.encoding = No se puede conectar: la ruta del socket de callosum no es UTF-8 válido{ $line_break }

journal.cli.host.health.socket.inspect = No se puede conectar: el socket de callosum no se puede inspeccionar: { $reason }{ $line_break }

journal.cli.host.health.socket.missing = No se puede conectar: no se encontró el socket de callosum en { $path }{ $line_break }

journal.cli.host.health.socket.type = la ruta no es un socket Unix

journal.cli.host.health.status.error = No se puede conectar: error de estado del supervisor: { $service }: { $reason }{ $line_break }

journal.cli.host.health.status.invalid = No se puede conectar: el supervisor envió una carga útil de estado no válida en { $path }

journal.cli.host.health.task.running = { $space_2 }{ $name } { $duration } s

journal.cli.host.health.task.slow = { $space_2 }{ $name } { $duration } s{ $space_2 }LENTO (límite { $limit } s)

journal.cli.host.health.task.stuck = { $space_2 }{ $name } { $duration } s{ $space_2 }ATASCADO (límite { $limit } s)

journal.cli.host.health.tasks = Tareas:{ $line_break }

journal.cli.host.health.tasks.empty = Tareas: ninguna{ $line_break }

journal.cli.host.health.timeout = se agotó el tiempo de espera del estado del supervisor (10 s){ $line_break }

journal.cli.host.health.timeout.malformed.frames = { $quantity ->
    [one] se agotó el tiempo de espera del estado del supervisor (10 s; se descartó { $count } trama malformada){ $line_break }
    [many] se agotó el tiempo de espera del estado del supervisor (10 s; se descartaron { $count } tramas malformadas){ $line_break }
   *[other] se agotó el tiempo de espera del estado del supervisor (10 s; se descartaron { $count } tramas malformadas){ $line_break }
    }

journal.cli.host.health.usage = uso: { $syntax }{ $line_break }

journal.cli.host.heartbeat.error = { $command }: { $detail }

journal.cli.host.heartbeat.help = uso: { $syntax }{ $line_break }{ $line_break }Ejecuta una pasada determinista de reparación del estado{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_5 }Ejecuta la revisión completa sin importar la antigüedad{ $line_break }

journal.cli.host.heartbeat.oplog = no se pudo crear el oplog de heartbeat: { $detail }

journal.cli.host.heartbeat.usage = uso: { $syntax }{ $line_break }

journal.cli.host.hosted.service.hosted.service.admission.refused = se rechazó la admisión del servicio hospedado { $service }: { $error }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.day = no se pudo mostrar el aviso del día de importación: { $error }{ $line_break }

journal.cli.host.importer.desktop.run.resolved.could.not.show.import.size = no se pudo mostrar el tamaño de la importación: { $error }{ $line_break }

journal.cli.host.install.models.coreml.disclosure.parakeet.assets.downloading.parakeet.tdt = recursos de parakeet: descargando el modelo { $model } { $literal_4 } ({ $literal_3 }) desde { $literal_2 }. consulta { $literal_1 }.

journal.cli.host.install.models.help = uso: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }{ $line_break }instala y verifica, de { $term }, los modelos de { $argument } incluidos ({ $argument_8 } local más recursos incluidos{ $line_break }de { $term_9 }{ $path }). la acción predeterminada revisa los artefactos locales de { $argument_8 } y{ $line_break }los obtiene si faltan; { $flag } vuelve a obtenerlos; { $flag_12 } solo verifica y sale{ $line_break }con un código distinto de cero ante cualquier problema.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_15 }verifica los recursos incluidos y los artefactos locales de { $argument_8 } sin{ $line_break }{ $spaces_24 }obtenerlos.{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_15 }ignora si ya está listo y vuelve a obtener{ $path_20 } los artefactos de { $argument_8 }{ $line_break }{ $spaces_24 }locales.{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_7 }instala o verifica solo los modelos incluidos necesarios para{ $line_break }{ $spaces_24 }que el diario esté listo; no obtengas proveedores opcionales.{ $line_break }{ $spaces_2 }{ $flag_23 } { $choices }{ $line_break }{ $spaces_24 }variante del diario a instalar o verificar. { $literal } respeta{ $line_break }{ $spaces_24 }{ $argument_26 } en { $literal_27 }{ $path_28 }, luego { $literal }detecta.{ $line_break }

journal.cli.host.install.models.install.required.rfdetr.rf.detr.install.unsupported.platform = instalación de { $literal_1 }: plataforma no compatible { $os }/{ $arch }; se omiten los recursos de detección de objetos de { $literal_1 }

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.percent = modelo de parakeet { $model_name }: { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.parakeet.model.model.name.verifying = modelo de parakeet { $model_name }: verificando la descarga

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.download.starting = configuración de parakeet: iniciando la descarga

journal.cli.host.install.models.install.with.live.progress.parakeet.setup.downloading.required.files = configuración de parakeet: descargando los archivos obligatorios { $percent }% ({ $received } / { $total } MB)

journal.cli.host.install.models.install.with.live.progress.preparing.parakeet.speech.model.model = preparando el modelo de habla de parakeet: { $model_name }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.model = la revisión de { $literal_1 } falló: falta el modelo en { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name = la revisión de { $literal_1 } falló: falta { $name } en { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.cpp.check.failed.name.not.executable.at.path = la revisión de { $literal_1 } falló: { $name } no es ejecutable en { $path }

journal.cli.host.install.models.parakeet.ready.parakeet.model.path.missing = falta la ruta del modelo de parakeet

journal.cli.host.install.models.parakeet.ready.parakeet.path.missing = falta la ruta de parakeet

journal.cli.host.install.models.parakeet.without.download.parakeet.install.unsupported.platform.os = instalación de parakeet: plataforma no compatible { $os }/{ $arch }; compatibles: { $literal_1 }, { $literal_2 }

journal.cli.host.install.models.parakeet.without.download.parakeet.speech.recognition.ships.inside = parakeet: el reconocimiento de voz viene dentro de la app del diario en windows, así que no hay nada que descargar

journal.cli.host.install.models.ready.line.model.ready.path = modelo listo: { $path }

journal.cli.host.install.models.resolve.variant.invalid.variant.cuda.use.cpu = { $literal_1 }={ $literal_2 } no válido; usa { $literal_3 } en { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.invalid.variant.value.use.cpu = { $literal_1 }='{ $value }' no válido; usa { $literal_3 } o { $literal_2 }

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on = la variante { $literal_1 } no es compatible con { $os }

journal.cli.host.install.models.resolve.variant.variant.coreml.not.supported.on.supported.on.os.arch = la variante { $literal_1 } no es compatible con { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on = la variante { $literal_1 } no es compatible con { $os }

journal.cli.host.install.models.resolve.variant.variant.cpu.not.supported.on.supported.on.os.arch = la variante { $literal_1 } no es compatible con { $os }/{ $arch }

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on = la variante { $literal_1 } no es compatible con { $os }

journal.cli.host.install.models.resolve.variant.variant.cuda.not.supported.on.supported.on.os.arch = la variante { $literal_1 } no es compatible con { $os }/{ $arch }

journal.cli.host.install.models.rfdetr.bundled.asset.disclosure.rf.detr.assets.verifying.bundled = recursos de rf-detr: verificando el motor { $literal_2 } { $version } incluido ({ $literal_3 }) y los pesos { $literal_4 } nano { $literal_5 } ({ $literal_3 }). consulta { $literal_1 }.

journal.cli.host.install.models.rfdetr.repair.object.detection.unavailable.repair.or = la detección de objetos no está disponible. repara o reinstala tu diario. { $error }

journal.cli.host.install.models.run.inner.speech.install.already.running.wait = la instalación de habla ya está en ejecución; espera a que termine

journal.cli.host.install.models.run.inner.with.bundled.asset.verification.failed = falló la verificación de los recursos incluidos: { $error }

journal.cli.host.install.models.run.inner.with.observing.parakeet.install.state.suffix = observando la instalación de parakeet: { $state }{ $suffix }

journal.cli.host.install.models.run.inner.with.parakeet.install.already.running.for = la instalación de parakeet ya está en ejecución para un destino distinto

journal.cli.host.install.models.run.inner.with.parakeet.install.failed = falló la instalación de parakeet

journal.cli.host.install.models.run.inner.with.timed.out.observing.parakeet.install = se agotó el tiempo al observar la instalación de parakeet

journal.cli.host.install.models.usage = uso: { $syntax }{ $line_break }{ $spaces_30 }{ $syntax_4 }{ $line_break }{ $spaces_30 }{ $syntax_5 }{ $line_break }

journal.cli.host.install.provider.dispatch.message.provider.install.failed = falló la instalación de { $provider }

journal.cli.host.install.provider.global.local.model.assets.downloading.llama = recursos del modelo local: descargando el entorno de ejecución de { $literal_4 } ({ $literal_7 }; la compilación { $literal_6 } también lleva componentes del entorno de ejecución con licencia de { $literal_5 }) y el modelo ({ $literal_3 }) desde { $literal_2 }. consulta { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.and = el razonamiento local descarga el modelo y el proyector de visión ({ $literal_3 }) desde { $literal_2 } cuando hace falta. el entorno de ejecución está incluido con el diario. consulta { $literal_1 }.

journal.cli.host.install.provider.global.local.thinking.downloads.model.apache = el razonamiento local descarga el modelo ({ $literal_3 }) desde { $literal_2 }. el entorno de ejecución de { $literal_4 } está incluido con el diario. consulta { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.and.its.speech = { $literal_2 } y su modelo de habla vienen con el diario en windows, así que no hay nada que instalar. consulta { $literal_1 }.

journal.cli.host.install.provider.global.parakeet.cpp.can.t.run = { $literal_1 } no puede ejecutarse en una mac. una mac transcribe con su modelo { $literal_2 } en su lugar.

journal.cli.host.install.provider.global.parakeet.cpp.fetches.two.artifacts = { $literal_4 } obtiene dos artefactos en la caché de proveedores de este diario antes de poder ejecutarse, ambos desde { $literal_2 }: el binario del servidor { $literal_3 } ({ $literal_6 }) y el modelo de habla ({ $literal_5 }). consulta { $literal_1 }.

journal.cli.host.install.provider.help = uso: { $syntax }{ $line_break }{ $line_break }Instala o reintenta un entorno de ejecución de proveedor.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Proveedor a instalar: '{ $literal }' o '{ $literal_7 }'.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.install.provider.local.target.sha.local.fingerprint.dispatch.omitted.target = el despacho de la huella local omitió target_fingerprint_sha256

journal.cli.host.install.provider.observe.existing.with.provider.install.already.running.for = la instalación de { $provider } ya está en ejecución para un destino distinto

journal.cli.host.install.provider.observe.existing.with.timed.out.observing.provider.install = se agotó el tiempo al observar la instalación de { $provider }

journal.cli.host.install.provider.progress.line.observing.provider.install.state.suffix = observando la instalación de { $provider }: { $state }{ $suffix }

journal.cli.host.install.provider.run.inner.with.parakeet.already.installed = parakeet ya está instalado

journal.cli.host.install.provider.run.inner.with.unsupported.provider.provider.supported.local = proveedor no compatible { $provider }; compatibles: { $literal_1 }

journal.cli.host.install.provider.run.local.inner.with.platform.and.target.sha.local.already.installed = local ya está instalado

journal.cli.host.install.provider.unavailable.status.error.could.not.read.persisted.provider = no se pudo leer el estado persistido de la instalación de { $provider }: { $error }

journal.cli.host.install.provider.usage = uso: { $syntax }{ $line_break }

journal.cli.host.installation.executable = no se pudo resolver el ejecutable actual: { $error }

journal.cli.host.installation.executable.parent = el ejecutable actual no tiene un directorio contenedor

journal.cli.host.installation.recovery = no se pudo verificar esta instalación.{ $line_break }ejecuta `{ $setup }` para revisarla. si la configuración inicial termina correctamente, vuelve a intentarlo.{ $line_break }detalles: { $detail }

journal.cli.host.installation.root = no se pudo resolver la raíz de identidad de la instalación desde { $directory }

journal.cli.host.logs.count = cantidad no válida: { $detail }

journal.cli.host.logs.follow.descriptor = el catálogo falló para { $path }: el descriptor del oplog cambió durante el seguimiento

journal.cli.host.logs.follow.empty = No se encontraron archivos de registro.

journal.cli.host.logs.follow.failed = { $operation } falló para { $path }

journal.cli.host.logs.follow.root = el catálogo falló para { $path }: la raíz del diario no está disponible

journal.cli.host.logs.follow.source = { $operation } falló para { $path }: { $source }

journal.cli.host.logs.grep.compile = no se pudo compilar el patrón de regex

journal.cli.host.logs.grep.invalid = patrón de regex no válido en el desplazamiento de byte { $offset }

journal.cli.host.logs.grep.unsupported = función de regex no compatible '{ $family }' en el desplazamiento de byte { $offset }

journal.cli.host.logs.health.error = registros de estado: { $detail }

journal.cli.host.logs.health.usage = { $command }: error: { $detail }

journal.cli.host.logs.service.empty = === registros del servicio === (sin hojas de oplog)

journal.cli.host.logs.service.error = registros del servicio: { $detail }

journal.cli.host.logs.service.heading = === registros del servicio ===

journal.cli.host.logs.service.operation = { $operation } falló: { $source }

journal.cli.host.logs.service.unsupported = Error: plataforma no compatible '{ $platform }'

journal.cli.host.logs.since.invalid = Tiempo no válido: '{ $input }'. Usa p. ej., { $examples }

journal.cli.host.logs.since.overflow = Entero de Python demasiado grande para convertirlo a entero de C

journal.cli.host.main.windows.service.admission.failed = falló la admisión del servicio de windows: { $error }

journal.cli.host.malformed.jsonl.warning.solstone.observe.utils.invalid.json.at.line.in = { $literal_warning_solstone_observe_utils }:No es válido el { $literal_json } en la línea { $line } de { $path }: { $error }

journal.cli.host.mcp.activity.activity = { $command_solstone_journal_mcp_activity }: { $error }

journal.cli.host.mcp.activity.activity.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_activity } no está compilado en esta compilación

journal.cli.host.mcp.activity.activity.unknown.outcome = { $command_solstone_journal_mcp_activity }: resultado desconocido { $value }

journal.cli.host.mcp.activity.activity.unknown.tool = { $command_solstone_journal_mcp_activity }: herramienta desconocida { $value }

journal.cli.host.mcp.activity.bytes = { $space_4 }bytes:{ $space_3 }{ $byte_count }

journal.cli.host.mcp.activity.created = { $space_4 }creado: { $created }

journal.cli.host.mcp.activity.help = uso: { $syntax }{ $line_break }{ $spaces_37 }{ $syntax_4 }{ $line_break }{ $spaces_37 }{ $syntax_5 }{ $line_break }{ $line_break }Muestra qué pidieron las conexiones y cómo terminó cada solicitud.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }solo una conexión de token de portador, por su etiqueta{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }solo una conexión de { $term }, por su id de cliente{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }con { $flag }, qué concesión, cuando un id de cliente tiene varias{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_10 }solo solicitudes a esta herramienta{ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_4 }solo solicitudes que terminaron así{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_2 }desde este día{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_4 }hasta este día, incluido{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_12 }como máximo N solicitudes, de 1 a 1000 (predeterminado: 20){ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_15 }imprime { $term_25 }{ $line_break }

journal.cli.host.mcp.activity.no.match = { $examined_quantity ->
    [one] Ninguna coincidencia en el { $examined } registro examinado; puede haber más hacia atrás.
    [many] Ninguna coincidencia en los { $examined } registros examinados; puede haber más hacia atrás.
    *[other] Ninguna coincidencia en los { $examined } registros examinados; puede haber más hacia atrás.
    }

journal.cli.host.mcp.activity.no.recorded.mcp.activity.matches = Ninguna actividad registrada de { $literal_mcp } coincide.

journal.cli.host.mcp.activity.origin = { $space_4 }origen:{ $space_2 }{ $origin }

journal.cli.host.mcp.activity.raise.limit.or.narrow.day.from.day.to.to.see = Aumenta { $literal_limit } o acota { $literal_day_from }/{ $literal_day_to } para ver más.

journal.cli.host.mcp.activity.reason = { $space_4 }motivo:{ $space_2 }{ $reason }

journal.cli.host.mcp.activity.record = { $space_4 }registro:{ $space_2 }{ $day }/{ $segment }

journal.cli.host.mcp.activity.request = { $space_4 }solicitud: { $rendered }

journal.cli.host.mcp.activity.request.arguments.exceeded.the.record.bound.and.were.not.stored = { $space_4 }solicitud: (los argumentos superaron el límite del registro y no se almacenaron)

journal.cli.host.mcp.activity.request.legacy = { $space_4 }solicitud: (esquema 1: no se registró)

journal.cli.host.mcp.activity.result = { $count_quantity ->
    [one] { $space_4 }resultado:{ $space_2 }{ $count } resultado, digest { $digest }
    [many] { $space_4 }resultado:{ $space_2 }{ $count } resultados, digest { $digest }
    *[other] { $space_4 }resultado:{ $space_2 }{ $count } resultados, digest { $digest }
    }

journal.cli.host.mcp.activity.result.targets = { $count_quantity ->
    [one] { $space_4 }resultado:{ $space_2 }{ $count } resultado, digest { $digest }, destinos { $targets }
    [many] { $space_4 }resultado:{ $space_2 }{ $count } resultados, digest { $digest }, destinos { $targets }
    *[other] { $space_4 }resultado:{ $space_2 }{ $count } resultados, digest { $digest }, destinos { $targets }
    }

journal.cli.host.mcp.activity.result.truncated = { $count_quantity ->
    [one] { $space_4 }resultado:{ $space_2 }{ $count } resultado, digest { $digest }, destinos { $targets }, ...
    [many] { $space_4 }resultado:{ $space_2 }{ $count } resultados, digest { $digest }, destinos { $targets }, ...
    *[other] { $space_4 }resultado:{ $space_2 }{ $count } resultados, digest { $digest }, destinos { $targets }, ...
    }

journal.cli.host.mcp.activity.schema.connection = (esquema 1: no se registró ninguna conexión)

journal.cli.host.mcp.activity.shown.complete = { $shown_quantity ->
    [one] { $shown } registro mostrado de { $examined } examinados. { $counts }
    [many] { $shown } registros mostrados de { $examined } examinados. { $counts }
    *[other] { $shown } registros mostrados de { $examined } examinados. { $counts }
    }

journal.cli.host.mcp.activity.shown.partial = { $shown_quantity ->
    [one] { $shown } registro mostrado de { $examined } examinados (puede haber más hacia atrás). { $counts }
    [many] { $shown } registros mostrados de { $examined } examinados (puede haber más hacia atrás). { $counts }
    *[other] { $shown } registros mostrados de { $examined } examinados (puede haber más hacia atrás). { $counts }
    }

journal.cli.host.mcp.activity.unreadable = { $count_quantity ->
    [one] no se pudo analizar { $count } registro del rango y no se incluye arriba.
    [many] no se pudieron analizar { $count } registros del rango y no se incluyen arriba.
    *[other] no se pudieron analizar { $count } registros del rango y no se incluyen arriba.
    }

journal.cli.host.mcp.help = uso: { $syntax }{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $line_break }subcomandos:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }Ejecuta el endpoint de { $term }{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_12 }Ejecuta la conexión para los agentes de esta computadora{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_16 }Muestra el estado del endpoint de { $term }{ $line_break }{ $spaces_2 }{ $subcommand_12 }{ $spaces_17 }Administra los tokens de portador de { $term }{ $line_break }{ $spaces_2 }{ $subcommand_14 }{ $spaces_15 }Administra el código de vinculación del agente{ $line_break }{ $spaces_2 }{ $subcommand_15 }{ $spaces_17 }Administra los clientes registrados de { $term_16 }{ $line_break }{ $spaces_2 }{ $subcommand_17 }{ $spaces_12 }Administra los permisos de lectura de la conexión{ $line_break }{ $spaces_2 }{ $subcommand_18 }{ $spaces_14 }Muestra qué pidieron las conexiones y cómo terminó cada solicitud{ $line_break }{ $spaces_2 }{ $subcommand_20 }{ $spaces_17 }Invoca una lectura de { $term } sin abrir un puerto{ $line_break }

journal.cli.host.mcp.local.door.help = uso: { $syntax }{ $line_break }{ $line_break }Ejecuta la conexión para los agentes de esta computadora.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.mcp.local.door.local.door.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_local_door } no está compilado en esta compilación

journal.cli.host.mcp.local.door.mcp.local.door.failed = falló mcp local-door: { $class }

journal.cli.host.mcp.oauth.error.detail = { $command_solstone_journal_mcp } { $command } { $operation }: { $error }

journal.cli.host.mcp.oauth.help = uso: { $syntax }{ $line_break }{ $line_break }Administra los clientes registrados de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $line_break }subcomandos:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Lista los clientes registrados de { $term }{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_6 }Revoca un cliente registrado de { $term }{ $line_break }

journal.cli.host.mcp.oauth.list.help = uso: { $syntax }{ $line_break }{ $line_break }Lista los clientes registrados de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.mcp.oauth.oauth.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_oauth } no está compilado en esta compilación

journal.cli.host.mcp.oauth.oauth.revoke.no.such.oauth.client = { $command_solstone_journal_mcp_oauth_revoke }: no existe ese cliente de { $literal_oauth }

journal.cli.host.mcp.oauth.revoke.help = uso: { $syntax }{ $line_break }{ $line_break }Revoca un cliente de { $term } registrado.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_13 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_2 }el cliente que se revoca{ $line_break }

journal.cli.host.mcp.oauth.revoked.oauth.client = Se revocó el cliente de { $literal_oauth } { $client_id }.

journal.cli.host.mcp.pairing.at.expires.at = Vence el { $expires_at }.

journal.cli.host.mcp.pairing.at.it.works.only.at.your.own.domain = Funciona solo en tu propio dominio.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer = Funciona solo para un agente en esta computadora.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer.to = Funciona solo para un agente en esta computadora. Para conectar un agente de otra forma, usa { $literal_door } lan, { $literal_solstone_me } o hostname.

journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.your.network = Funciona solo para un agente en tu red.

journal.cli.host.mcp.pairing.at.it.works.only.through.solstone.me = Funciona solo a través de { $literal_solstone_me }.

journal.cli.host.mcp.pairing.at.pairing.generate.no.hostname.is.set.up.for.this.journal = { $command_solstone_journal_mcp_pairing_generate }: no hay un nombre de host configurado para este diario. configúralo primero en la app de agentes.

journal.cli.host.mcp.pairing.at.pairing.generate.this.journal.s.config.could.not.be.read = { $command_solstone_journal_mcp_pairing_generate }: no se pudo leer la configuración de este diario.

journal.cli.host.mcp.pairing.at.pairing.revoke.no.active.pairing.code.to.revoke = { $command_solstone_journal_mcp_pairing_revoke }: no hay un código de vinculación activo para revocar

journal.cli.host.mcp.pairing.at.revoked.the.active.pairing.code = Se revocó el código de vinculación activo.

journal.cli.host.mcp.pairing.at.save.this.now.it.will.not.be.shown.again.and = Guarda esto ahora. No se volverá a mostrar y no se puede recuperar:

journal.cli.host.mcp.pairing.generate.help = uso: { $syntax }{ $line_break }{ $line_break }Genera un código de vinculación para un agente, válido por 10 minutos y un solo uso. Se muestra una vez.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $flag } { $choices }{ $line_break }{ $spaces_24 }dónde funciona el código (predeterminado: { $literal }, un agente en esta computadora){ $line_break }

journal.cli.host.mcp.pairing.generated = Se generó un código de vinculación, válido por 10 minutos y un solo uso.

journal.cli.host.mcp.pairing.help = uso: { $syntax }{ $line_break }{ $line_break }Gestiona el código de vinculación del agente.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $line_break }subcomandos:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_4 }Genera un código de vinculación{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_6 }Revoca el código de vinculación actual{ $line_break }

journal.cli.host.mcp.pairing.pairing.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_pairing } no está compilado en esta compilación

journal.cli.host.mcp.pairing.revoke.help = uso: { $syntax }{ $line_break }{ $line_break }Revoca el código de vinculación actual.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.mcp.permission.categories = Categorías: { $categories }

journal.cli.host.mcp.permission.clear.help = uso: { $syntax }{ $line_break }{ $line_break }Quita el permiso de lectura de una conexión.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }una conexión de token bearer, por su etiqueta{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }una conexión de { $term }, por su id de cliente{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }con { $flag }, qué concesión, cuando un id de cliente tiene varias{ $line_break }

journal.cli.host.mcp.permission.cleared.permission.for.connection = Se quitó el permiso de la conexión { $connection }.

journal.cli.host.mcp.permission.connection = Conexión: { $connection }

journal.cli.host.mcp.permission.evaluation = Evaluación: { $evaluation }

journal.cli.host.mcp.permission.generation = Generación: { $generation }

journal.cli.host.mcp.permission.help = uso: { $syntax }{ $line_break }{ $line_break }Gestiona los permisos de lectura de las conexiones.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $line_break }subcomandos:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_8 }Muestra los permisos de lectura, de todas las conexiones o de una{ $line_break }{ $spaces_2 }{ $subcommand_7 }{ $spaces_9 }Establece qué puede leer una conexión{ $line_break }{ $spaces_2 }{ $subcommand_9 }{ $spaces_7 }Quita el permiso de lectura de una conexión{ $line_break }

journal.cli.host.mcp.permission.no.mcp.connection.permissions.configured = No hay permisos de conexión de { $literal_mcp } configurados.

journal.cli.host.mcp.permission.no.permission.was.configured.for.connection = No se configuró un permiso para la conexión { $connection }.

journal.cli.host.mcp.permission.permission.clear = { $command_solstone_journal_mcp_permission_clear }: { $error }

journal.cli.host.mcp.permission.permission.management.is.not.compiled.into.this.build = la gestión de { $command_solstone_journal_mcp_permission } no está compilada en esta compilación

journal.cli.host.mcp.permission.permission.no.permission.configured = Permiso: no hay un permiso configurado

journal.cli.host.mcp.permission.permission.set = { $command_solstone_journal_mcp_permission_set }: { $error }

journal.cli.host.mcp.permission.permission.set.choose.a.category.from = { $command_solstone_journal_mcp_permission_set }: elige una categoría de { $categories }

journal.cli.host.mcp.permission.permission.set.choose.places.precision = { $command_solstone_journal_mcp_permission_set }: elige un nivel de detalle de lugares de { $levels }

journal.cli.host.mcp.permission.permission.set.places.precision.requires.places = { $command_solstone_journal_mcp_permission_set }: { $flag } requiere { $category_flag }

journal.cli.host.mcp.permission.permission.show = { $command_solstone_journal_mcp_permission_show }: { $error }

journal.cli.host.mcp.permission.places.precision = Detalle de lugares: { $precision }

journal.cli.host.mcp.permission.set.help.places.precision = { $spaces_2 }{ $option }{ $spaces_2 }detalle de la ubicación cuando se usa { $flag }: { $values } (predeterminado: { $default }){ $line_break }

journal.cli.host.mcp.permission.set.help.v2 = uso: { $syntax }{ $line_break }{ $spaces_43 }{ $syntax_4 }{ $line_break }{ $spaces_43 }{ $syntax_5 }{ $line_break }{ $line_break }Establece qué puede leer una conexión.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_8 }una conexión de token bearer, por su etiqueta{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }una conexión de { $term }, por su id de cliente{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_2 }con { $flag }, qué concesión, cuando un id de cliente tiene varias{ $line_break }{ $spaces_2 }{ $option_16 }{ $spaces_2 }un tipo que esta conexión puede leer (repite para cada tipo){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_8 }solo esta faceta, por nombre; repite para más (predeterminado: el diario completo){ $line_break }

journal.cli.host.mcp.permission.set.mcp.read.permission.for.connection.generation = Se estableció el permiso de lectura de { $literal_mcp } para la conexión { $connection } (generación { $generation }).

journal.cli.host.mcp.permission.show.help = uso: { $syntax }{ $line_break }{ $line_break }Muestra los permisos de lectura, de todas las conexiones o de una.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_8 }una conexión de token bearer, por su etiqueta{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_4 }una conexión de { $term }, por su id de cliente{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_2 }con { $flag }, qué concesión, cuando un id de cliente tiene varias{ $line_break }

journal.cli.host.mcp.probe.help = uso: { $syntax }{ $line_break }{ $spaces_34 }{ $flag } { $argument } { $flag_6 } { $term }{ $line_break }{ $line_break }Invoca una lectura de { $term_8 } como una conexión, sin abrir un puerto.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_11 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_8 }una conexión de token bearer, por su etiqueta{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_4 }una conexión de { $term_16 }, por su id de cliente{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_2 }con { $flag_18 }, qué concesión, cuando un id de cliente tiene varias{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_10 }la herramienta que se llama{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_5 }los argumentos de la herramienta, como un objeto de { $term }{ $line_break }

journal.cli.host.mcp.service.help = uso: { $syntax }{ $line_break }{ $line_break }Ejecuta el endpoint de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.mcp.service.mcp.service.failed = falló el servicio de mcp: { $class }

journal.cli.host.mcp.service.service.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_service } no está compilado en esta compilación

journal.cli.host.mcp.status.capability.compiled.in.true = capacidad compilada: verdadero

journal.cli.host.mcp.status.capability.disabled = capacidad: desactivada

journal.cli.host.mcp.status.capability.enabled = capacidad: activada

journal.cli.host.mcp.status.capability.error = capacidad: error

journal.cli.host.mcp.status.help = uso: { $syntax }{ $line_break }{ $line_break }Muestra el estado del endpoint de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.mcp.status.lan.disabled = puerta lan: desactivada en el puerto { $port }

journal.cli.host.mcp.status.lan.enabled = puerta lan: activada en el puerto { $port }

journal.cli.host.mcp.status.local.disabled = puerta local: no activada en { $resource }

journal.cli.host.mcp.status.local.enabled = puerta local: activada en { $resource }

journal.cli.host.mcp.status.mcp.capability.config.status.only.this.does.not.report.service = solo estado de capacidad y configuración de { $literal_mcp }; esto no informa si el servicio está en ejecución.

journal.cli.host.mcp.status.status.could.not.read.bearer.token.ledger = { $command_solstone_journal_mcp_status }: no se pudo leer el registro de tokens bearer: { $error }

journal.cli.host.mcp.status.status.could.not.read.mcp.capability.configuration = { $command_solstone_journal_mcp_status }: no se pudo leer la configuración de capacidad de { $literal_mcp }: { $error }

journal.cli.host.mcp.status.status.is.not.compiled.into.this.build = { $command_solstone_journal_mcp_status } no está compilado en esta compilación

journal.cli.host.mcp.status.status.mcp.capability.configuration.is.invalid = { $command_solstone_journal_mcp_status }: la configuración de capacidad de { $literal_mcp } no es válida: { $error }

journal.cli.host.mcp.status.token.count = cantidad de tokens: { $count }

journal.cli.host.mcp.status.token.unavailable = cantidad de tokens: no disponible

journal.cli.host.mcp.token.create.help = uso: { $syntax }{ $line_break }{ $line_break }Crea un token bearer para una conexión. El token se muestra una vez.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }un nombre para la conexión a la que corresponde el token{ $line_break }

journal.cli.host.mcp.token.created.bearer.token.for.label = Se creó un token bearer para la etiqueta { $label }.

journal.cli.host.mcp.token.grant.before.the.agent.connects.an.agent.that.connected.first = Concede el acceso antes de que el agente se conecte. Un agente que se conectó primero conserva su lista de herramientas anterior hasta que se vuelva a conectar.

journal.cli.host.mcp.token.help = uso: { $syntax }{ $line_break }{ $line_break }Gestiona los tokens bearer de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $line_break }subcomandos:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_6 }Crea un token bearer para una conexión{ $line_break }{ $spaces_2 }{ $subcommand_8 }{ $spaces_8 }Lista los tokens{ $line_break }{ $spaces_2 }{ $subcommand_10 }{ $spaces_6 }Revoca un token{ $line_break }

journal.cli.host.mcp.token.list.help = uso: { $syntax }{ $line_break }{ $line_break }Lista la etiqueta de cada token y cuándo se creó.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.mcp.token.revoke.help = uso: { $syntax }{ $line_break }{ $line_break }Revoca el token bearer de una etiqueta.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }la etiqueta del token que se revoca{ $line_break }

journal.cli.host.mcp.token.revoked.bearer.token.for.label = Se revocó el token bearer de la etiqueta { $label }.

journal.cli.host.mcp.token.save.this.token.now.it.will.not.be.shown.again = Guarda este token ahora. No se volverá a mostrar y no se puede recuperar:

journal.cli.host.mcp.token.token.management.is.not.compiled.into.this.build = la gestión de { $command_solstone_journal_mcp_token } no está compilada en esta compilación

journal.cli.host.mcp.token.until.you.grant.it.read.access.an.agent.using.this = Hasta que le concedas acceso de lectura, un agente que use este token solo ve { $literal_save_memory } y { $literal_recall_memory }. Para permitirle leer tu diario completo (agrega { $literal_facet } NAME para permitir solo esa faceta):

journal.cli.host.mcp.usage = uso: { $syntax }{ $line_break }

journal.cli.host.navigate.error.is.no.longer.supported.put.facet.selection.in.the.destination.url.for = { $command }: error: { $option } ya no es compatible. Pon la selección de faceta en el { $term } de destino; por ejemplo, { $path }

journal.cli.host.navigate.help = uso: { $syntax }{ $line_break }{ $line_break }Navega el navegador a una ruta.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_18 }ruta de { $term } a la que se navega.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.navigate.print.navigation.navigate.path = Navegar: { $path }

journal.cli.host.navigate.run.navigate.callosum.socket.unavailable.path = solstone journal navigate: error: socket de { $literal_1 } no disponible: { $path }

journal.cli.host.navigate.run.navigate.failed.encode.callosum.request = solstone journal navigate: error: no se pudo codificar la solicitud de { $literal_1 }: { $error }

journal.cli.host.navigate.usage = uso: { $syntax }{ $line_break }

journal.cli.host.open.help = uso: { $syntax }{ $line_break }{ $line_break }abre tu diario en tu navegador con un enlace de un solo uso.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_10 }página que se abre (predeterminado: /){ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_4 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_3 }imprime el enlace de un solo uso en lugar de abrir un navegador{ $line_break }

journal.cli.host.open.mint.could.not.make.link = no se pudo crear un enlace de tu diario: { $error }

journal.cli.host.open.mint.could.not.make.link.try = no se pudo crear un enlace de tu diario. vuelve a intentarlo.

journal.cli.host.open.mint.response.from.could.not.be = no se pudo leer la respuesta de tu diario. vuelve a intentarlo.

journal.cli.host.open.run.browser.could.not.be.opened = no se pudo abrir un navegador aquí. ejecuta este comando de nuevo con { $literal_1 } y abre el enlace que imprime en el plazo de un minuto.

journal.cli.host.open.run.open.it.from.another.computer = para abrirlo desde otra computadora, primero reenvía los puertos del diario por { $literal_1 } y deja esa terminal abierta:

journal.cli.host.open.run.open.url.path = abre tu diario: { $url }{ $path }

journal.cli.host.open.socket.linux.down.isn.t.running.for.this = tu diario no está en ejecución para esta cuenta en esta computadora, así que no hay nada que abrir. inícialo con { $literal_1 }.

journal.cli.host.open.socket.linux.old.running.older.than.this.command = el diario en ejecución es más antiguo que este comando. reinícialo con { $literal_1 }, luego con { $literal_2 }, y vuelve a intentarlo.

journal.cli.host.open.socket.mac.down.isn.t.running.for.this = tu diario no está en ejecución para esta cuenta en esta computadora, así que no hay nada que abrir. abre la app del diario para iniciarlo.

journal.cli.host.open.socket.mac.old.running.older.than.this.command = el diario en ejecución es más antiguo que este comando. cierra la app del diario, ábrela de nuevo y vuelve a intentarlo.

journal.cli.host.open.socket.windows.down.isn.t.running.for.this = tu diario no está en ejecución para esta cuenta en esta computadora, así que no hay nada que abrir. inícialo desde la app del diario y vuelve a intentarlo.

journal.cli.host.open.socket.windows.old.running.older.than.this.command = el diario en ejecución es más antiguo que este comando. reinícialo desde la app del diario y vuelve a intentarlo.

journal.cli.host.open.usage = uso: { $syntax }{ $line_break }

journal.cli.host.parser.convey.argument.journal.expected.one.argument = { $command }: error: argumento { $flag }: se esperaba un argumento

journal.cli.host.parser.convey.argument.port.expected.one.argument = { $command }: error: argumento { $flag }: se esperaba un argumento

journal.cli.host.parser.convey.argument.port.invalid.int.value = { $command }: error: argumento { $flag }: entero no válido

journal.cli.host.parser.convey.argument.port.invalid.int.value.value = { $command }: error: argumento { $flag }: entero no válido: '{ $value }'

journal.cli.host.parser.convey.argument.port.must.be.between.n1.and.n65535 = { $command }: error: argumento { $flag }: debe estar entre 1 y 65535

journal.cli.host.parser.convey.the.following.arguments.are.required.port = { $command }: error: los siguientes argumentos son obligatorios: { $flag }

journal.cli.host.parser.convey.unrecognized.arguments.arguments = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.host.parser.cortex.unrecognized.arguments.arguments = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.host.parser.direct.port.value.argument.direct.port.invalid.int.value.value = { $command }: error: argumento { $flag }: entero no válido: '{ $value }'

journal.cli.host.parser.direct.port.value.argument.direct.port.must.be.between.n1.and.n65535 = { $command }: error: argumento { $flag }: debe estar entre 1 y 65535

journal.cli.host.parser.grab.argument.out.expected.one.argument = { $command }: error: argumento { $flag }: se esperaba un argumento

journal.cli.host.parser.grab.path.tokens.must.be.valid.unicode = { $command }: error: los tokens de ruta deben ser Unicode válido

journal.cli.host.parser.grab.unrecognized.arguments.arguments = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.host.parser.mcp.activity.expected.argument.after.name = { $command }: error: se esperaba un argumento después de { $name }

journal.cli.host.parser.mcp.activity.limit.must.be.between.n1.and.n1000 = { $command }: error: { $flag } debe estar entre 1 y 1000

journal.cli.host.parser.mcp.activity.must.be.nonempty.name = { $command }: error: { $name } no puede estar vacío

journal.cli.host.parser.mcp.activity.unknown.option.accepted.token.oauth.created.tool.outcome.day.from.day.to.limit.json.other = { $command }: error: opción desconocida { $other }; se aceptan: { $flag }, { $flag_2 }, { $flag_3 }, { $flag_4 }, { $flag_5 }, { $flag_6 }, { $flag_7 }, { $flag_8 }, { $flag_9 }

journal.cli.host.parser.mcp.invalid.mcp.command.value = { $command }: error: comando de { $term } no válido: { $value }

journal.cli.host.parser.mcp.oauth.client.id.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: error: client-id debe ser { $term } válido y no estar vacío

journal.cli.host.parser.mcp.oauth.client.id.expected.exactly.one.client.id.client.id.argument = { $command }: error: se esperaba exactamente un argumento { $flag } { $argument }

journal.cli.host.parser.mcp.oauth.client.id.the.following.arguments.are.required.client.id.client.id = { $command }: error: los siguientes argumentos son obligatorios: { $flag } { $argument }

journal.cli.host.parser.mcp.oauth.invalid.mcp.oauth.command.value = { $command }: error: comando oauth de { $term } no válido: { $value }

journal.cli.host.parser.mcp.oauth.the.following.arguments.are.required.list.revoke = { $command }: error: los siguientes argumentos son obligatorios: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.expected.optional.door.local.lan.solstone.me.hostname.argument = { $command }: error: se esperaba un argumento opcional { $syntax }

journal.cli.host.parser.mcp.pairing.invalid.mcp.pairing.command.value = { $command }: error: comando pairing de { $term } no válido: { $value }

journal.cli.host.parser.mcp.pairing.the.following.arguments.are.required.generate.revoke = { $command }: error: los siguientes argumentos son obligatorios: { $literal }, { $literal_2 }

journal.cli.host.parser.mcp.pairing.unknown.door.choose.one.of.local.lan.solstone.me.hostname = { $command }: error: puerta desconocida, elige una de { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }

journal.cli.host.parser.mcp.permission.invalid.mcp.permission.command.value = { $command }: error: comando permission de { $term } no válido: { $value }

journal.cli.host.parser.mcp.permission.set.at.least.one.category.is.required = { $command }: error: se requiere al menos un { $flag }

journal.cli.host.parser.mcp.permission.set.category.must.be.nonempty = { $command }: error: la categoría no puede estar vacía

journal.cli.host.parser.mcp.permission.set.category.must.be.valid.utf.n8.and.nonempty = { $command }: error: la categoría debe ser { $term } válido y no estar vacía

journal.cli.host.parser.mcp.permission.set.expected.argument.after.category = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.permission.set.expected.argument.after.facet = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.permission.set.expected.argument.after.places.precision = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.nonempty = { $command }: error: el nombre de la faceta no puede estar vacío

journal.cli.host.parser.mcp.permission.set.facet.name.must.be.valid.utf.n8.and.nonempty = { $command }: error: el nombre de la faceta debe ser { $term } válido y no estar vacío

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.nonempty = { $command }: error: la precisión de lugares no puede estar vacía

journal.cli.host.parser.mcp.permission.set.places.precision.must.be.valid.utf.n8.and.nonempty = { $command }: error: la precisión de lugares debe ser { $term } válido y no estar vacía

journal.cli.host.parser.mcp.permission.set.places.precision.repeated = { $command }: error: { $flag } solo se puede indicar una vez

journal.cli.host.parser.mcp.permission.the.following.arguments.are.required.show.set.clear = { $command }: error: los siguientes argumentos son obligatorios: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.mcp.probe.arguments.is.required = { $command }: error: { $flag } es obligatorio

journal.cli.host.parser.mcp.probe.expected.argument.after.arguments = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.probe.expected.argument.after.tool = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.probe.tool.is.required = { $command }: error: { $flag } es obligatorio

journal.cli.host.parser.mcp.target.created.cannot.be.used.with.token = { $command }: error: { $flag } no se puede usar con { $flag_2 }

journal.cli.host.parser.mcp.target.created.timestamp.must.be.nonempty = { $command }: error: la marca de tiempo de creación no puede estar vacía

journal.cli.host.parser.mcp.target.created.timestamp.must.be.valid.utf.n8.and.nonempty = { $command }: error: la marca de tiempo de creación debe ser { $term } válido y no estar vacía

journal.cli.host.parser.mcp.target.expected.argument.after.created = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.target.expected.argument.after.oauth = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.target.expected.argument.after.token = { $command }: error: se esperaba un argumento después de { $flag }

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.nonempty = { $command }: error: el client-id de oauth no puede estar vacío

journal.cli.host.parser.mcp.target.oauth.client.id.must.be.valid.utf.n8.and.nonempty = { $command }: error: el client-id de oauth debe ser { $term } válido y no estar vacío

journal.cli.host.parser.mcp.target.one.of.token.or.oauth.is.required = { $command }: error: se requiere uno de { $flag } o { $flag_2 }

journal.cli.host.parser.mcp.target.specify.either.token.or.oauth.not.both = { $command }: error: indica { $flag } o { $flag_2 }, no ambos

journal.cli.host.parser.mcp.target.token.label.must.be.nonempty = { $command }: error: la etiqueta del token no puede estar vacía

journal.cli.host.parser.mcp.target.token.label.must.be.valid.utf.n8.and.nonempty = { $command }: error: la etiqueta del token debe ser { $term } válido y no estar vacía

journal.cli.host.parser.mcp.target.unexpected.argument.option = { $command }: error: argumento inesperado: { $option }

journal.cli.host.parser.mcp.the.following.arguments.are.required.service.local.door.status.token.pairing.oauth.permission.activity.probe = { $command }: error: los siguientes argumentos son obligatorios: { $literal }, { $literal_2 }, { $literal_3 }, { $literal_4 }, { $literal_5 }, { $literal_6 }, { $literal_7 }, { $literal_8 }, { $literal_9 }

journal.cli.host.parser.mcp.token.invalid.mcp.token.command.value = { $command }: error: comando token de { $term } no válido: { $value }

journal.cli.host.parser.mcp.token.label.expected.exactly.one.label.label.argument = { $command }: error: se esperaba exactamente un argumento { $flag } { $argument }

journal.cli.host.parser.mcp.token.label.label.must.be.valid.utf.n8.and.nonempty = { $command }: error: la etiqueta debe ser { $term } válido y no estar vacía

journal.cli.host.parser.mcp.token.label.the.following.arguments.are.required.label.label = { $command }: error: los siguientes argumentos son obligatorios: { $flag } { $argument }

journal.cli.host.parser.mcp.token.the.following.arguments.are.required.create.list.revoke = { $command }: error: los siguientes argumentos son obligatorios: { $literal }, { $literal_2 }, { $literal_3 }

journal.cli.host.parser.schedule.unrecognized.arguments.arguments = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.host.parser.spl.invalid.choice.choose.from.service.value = { $command }: error: opción no válida: { $value } (elige entre { $literal })

journal.cli.host.parser.spl.the.following.arguments.are.required.service = { $command }: error: los siguientes argumentos son obligatorios: { $literal }

journal.cli.host.parser.spl.unrecognized.arguments.arguments = { $command }: error: argumentos no reconocidos: { $arguments }

journal.cli.host.parser.supervisor.argument.direct.port.cannot.be.repeated = { $command }: error: argumento { $flag }: no se puede repetir

journal.cli.host.parser.supervisor.argument.direct.port.expected.one.argument = { $command }: error: argumento { $flag }: se esperaba un argumento

journal.cli.host.parser.supervisor.argument.direct.port.invalid.int.value = { $command }: error: argumento { $flag }: entero no válido

journal.cli.host.parser.validate.day.day.must.be.eight.digits.yyyymmdd.got.value = { $command }: error: el día debe tener ocho dígitos ({ $argument }), se recibió { $value }

journal.cli.host.process.admission = falló la admisión del proceso del diario: { $error }

journal.cli.host.read.error.error.solstone.observe.utils.error.reading = { $literal_error_solstone_observe_utils }:Error al leer { $path }: { $error }

journal.cli.host.render.oauth.store.error.no.active.pairing.code = { $command_solstone_journal_mcp } { $command } { $operation }: no hay un código de vinculación activo

journal.cli.host.render.oauth.store.error.no.such.oauth.client = { $command_solstone_journal_mcp } { $command } { $operation }: no existe ese cliente de { $literal_oauth }

journal.cli.host.render.supervisor.host.outcome.supervisor.failed.to.boot = no se pudo arrancar el supervisor: { $reason }

journal.cli.host.render.supervisor.host.outcome.supervisor.lifecycle.shutdown.failed.cause.readiness.self.heartbeat.identity = falló el cierre del ciclo de vida del supervisor: cause={ $cause }, readiness={ $readiness }, self_heartbeat={ $self_heartbeat }, identity={ $identity }

journal.cli.host.render.token.store.error.token = { $command_solstone_journal_mcp_token } { $operation }: { $error }

journal.cli.host.render.token.store.error.token.a.bearer.token.already.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: ya existe un token bearer para la etiqueta { $label }

journal.cli.host.render.token.store.error.token.invalid.label = { $command_solstone_journal_mcp_token } { $operation }: etiqueta no válida: { $error }

journal.cli.host.render.token.store.error.token.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp_token } { $operation }: no existe un token bearer para la etiqueta { $label }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.oauth.store = { $command_solstone_journal_mcp }: no se pudo consultar el almacén de oauth: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.token.store = { $command_solstone_journal_mcp }: no se pudo consultar el almacén de tokens: { $e }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.multiple.oauth.grants.matched.client.id.specify = { $command_solstone_journal_mcp }: varias concesiones de { $literal_oauth } coincidieron con { $literal_client_id } { $client_id }; indica { $literal_created } { $timestamp_arg }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.bearer.token.exists.for.label = { $command_solstone_journal_mcp }: no existe un token bearer para la etiqueta { $label }

journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.oauth.grant.found.matching.client.id = { $command_solstone_journal_mcp }: no se encontró una concesión de { $literal_oauth } que coincida con { $literal_client_id } { $client_id }

journal.cli.host.schedule.help = uso: { $syntax }{ $line_break }{ $line_break }Muestra las tareas programadas{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Activa el registro de depuración{ $line_break }

journal.cli.host.schedule.usage = uso: { $syntax }{ $line_break }

journal.cli.host.sense.help = uso: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }{ $line_break }Procesador unificado de archivos de observe{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_13 }Procesa archivos de un día específico (formato { $argument }) en lugar de vigilar{ $line_break }{ $spaces_2 }{ $option_12 }{ $spaces_2 }Máximo de trabajos simultáneos de descripción de pantalla al usar { $flag } (predeterminado: 1).{ $line_break }{ $spaces_2 }{ $flag_14 } { $choices }{ $line_break }{ $spaces_24 }Elimina las salidas de pantalla o de audio existentes y vuelve a procesar (requiere { $flag }){ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_5 }Filtra a un segmento específico (formato { $argument_19 }, requiere { $flag }){ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_7 }Filtra a un flujo específico (requiere { $flag }){ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_13 }Muestra lo que se procesaría (o se eliminaría con { $flag_14 }) sin hacer cambios{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_9 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_11 }Activa el registro de depuración{ $line_break }

journal.cli.host.sense.sense.batch.failed = falló el lote de sense: { $error }

journal.cli.host.sense.sense.parent.loss.admission.failed = falló la admisión de pérdida del padre de sense: { $error }

journal.cli.host.sense.service.sense.service.failed = falló el servicio de sense: { $class }

journal.cli.host.sense.usage = uso: { $syntax }{ $line_break }{ $spaces_20 }{ $syntax_4 }{ $line_break }{ $spaces_20 }{ $syntax_5 }{ $line_break }

journal.cli.host.service.argument.port = error: argumento inesperado '{ $text }'; pasa el puerto como { $flag } { $port }

journal.cli.host.service.argument.range = error: argumento inesperado '{ $text }'; pasa un puerto entre 1 y 65535 como { $flag } { $argument }

journal.cli.host.service.argument.unexpected = error: argumento inesperado '{ $text }'; pasa el puerto como { $flag } { $argument }

journal.cli.host.service.conflicting.absent.systemd.state = estado ausente de { $systemd } en conflicto

journal.cli.host.service.could.not.clear.the.service.s.ready.state = no se pudo borrar el estado de listo del servicio: { $error }

journal.cli.host.service.could.not.find.the.solstone.installation.from = no se pudo encontrar la instalación de { $solstone } desde { $path }

journal.cli.host.service.could.not.resolve.journal = no se pudo resolver el diario

journal.cli.host.service.could.not.resolve.the.solstone.executable = no se pudo resolver el ejecutable de { $solstone }: { $error }

journal.cli.host.service.could.not.verify.the.saved.installation = no se pudo verificar la instalación guardada: { $error }

journal.cli.host.service.could.not.verify.the.solstone.installation = no se pudo verificar la instalación de { $solstone }: { $error }

journal.cli.host.service.could.not.verify.the.solstone.setup.storage = no se pudo verificar el almacenamiento de configuración inicial de { $solstone }: { $error }

journal.cli.host.service.couldn.t.confirm.that.the.service.uses.this.journal = no se pudo confirmar que el servicio usa este diario. ejecuta '{ $setup }' para actualizar la ubicación guardada.

journal.cli.host.service.create.service.directory = crear el directorio del servicio: { $source }

journal.cli.host.service.create.service.health.directory = crear el directorio de estado del servicio: { $source }

journal.cli.host.service.duplicate.systemd.property = propiedad duplicada de { $systemd }: { $key }

journal.cli.host.service.entry.is.not.a.regular.file = la entrada no es un archivo regular

journal.cli.host.service.error.details = error: { $detail }

journal.cli.host.service.executable.parent = el ejecutable del servicio no tiene padre

journal.cli.host.service.file.absent = { $operation ->
    [change-journal-folder] se rechazó cambiar la carpeta del diario { $path }: ausente
    [install] se rechazó instalar el servicio { $path }: ausente
    [operate] se rechazó operar el servicio { $path }: ausente
    [restart] se rechazó reiniciar el servicio { $path }: ausente
    [route-repair] se rechazó reparar la ruta del servicio { $path }: ausente
    [status] se rechazó el estado del servicio { $path }: ausente
    [stop] se rechazó detener el servicio { $path }: ausente
   *[uninstall] se rechazó desinstalar el servicio { $path }: ausente
}

journal.cli.host.service.file.foreign = { $operation ->
    [change-journal-folder] se rechazó cambiar la carpeta del diario { $path }: no se reconoce como un archivo de servicio creado por el diario
    [install] se rechazó instalar el servicio { $path }: no se reconoce como un archivo de servicio creado por el diario
    [operate] se rechazó operar el servicio { $path }: no se reconoce como un archivo de servicio creado por el diario
    [restart] se rechazó reiniciar el servicio { $path }: no se reconoce como un archivo de servicio creado por el diario
    [route-repair] se rechazó reparar la ruta del servicio { $path }: no se reconoce como un archivo de servicio creado por el diario
    [status] se rechazó el estado del servicio { $path }: no se reconoce como un archivo de servicio creado por el diario
    [stop] se rechazó detener el servicio { $path }: no se reconoce como un archivo de servicio creado por el diario
   *[uninstall] se rechazó desinstalar el servicio { $path }: no se reconoce como un archivo de servicio creado por el diario
}

journal.cli.host.service.file.managed = { $operation ->
    [change-journal-folder] se rechazó cambiar la carpeta del diario { $path }: gestionado
    [install] se rechazó instalar el servicio { $path }: gestionado
    [operate] se rechazó operar el servicio { $path }: gestionado
    [restart] se rechazó reiniciar el servicio { $path }: gestionado
    [route-repair] se rechazó reparar la ruta del servicio { $path }: gestionado
    [status] se rechazó el estado del servicio { $path }: gestionado
    [stop] se rechazó detener el servicio { $path }: gestionado
   *[uninstall] se rechazó desinstalar el servicio { $path }: gestionado
}

journal.cli.host.service.file.unknown = { $operation ->
    [change-journal-folder] se rechazó cambiar la carpeta del diario { $path }: no se pudo verificar el archivo de servicio: { $cause }
    [install] se rechazó instalar el servicio { $path }: no se pudo verificar el archivo de servicio: { $cause }
    [operate] se rechazó operar el servicio { $path }: no se pudo verificar el archivo de servicio: { $cause }
    [restart] se rechazó reiniciar el servicio { $path }: no se pudo verificar el archivo de servicio: { $cause }
    [route-repair] se rechazó reparar la ruta del servicio { $path }: no se pudo verificar el archivo de servicio: { $cause }
    [status] se rechazó el estado del servicio { $path }: no se pudo verificar el archivo de servicio: { $cause }
    [stop] se rechazó detener el servicio { $path }: no se pudo verificar el archivo de servicio: { $cause }
   *[uninstall] se rechazó desinstalar el servicio { $path }: no se pudo verificar el archivo de servicio: { $cause }
}

journal.cli.host.service.incomplete.systemd.property.set = conjunto incompleto de propiedades de { $systemd }

journal.cli.host.service.inspect.launchd.directory = inspeccionar el directorio de { $launchd }: { $error }

journal.cli.host.service.inspect.service.directory = inspeccionar el directorio del servicio: { $error }

journal.cli.host.service.inspect.service.health.directory = inspeccionar el directorio de estado del servicio: { $error }

journal.cli.host.service.inspect.service.journal.root = inspeccionar la raíz del diario del servicio: { $error }

journal.cli.host.service.inspect.stale.launchd.unit = inspeccionar la unidad desactualizada de { $launchd }: { $error }

journal.cli.host.service.installation.changed.while.waiting.for.the.new.journal.folder = la instalación cambió mientras se esperaba la nueva carpeta del diario

journal.cli.host.service.launchd.accepted.the.unload.request.but.the.service.is = { $launchd } aceptó la solicitud de descarga, pero el servicio sigue presente

journal.cli.host.service.launchd.directory.is.not.a.real.directory = el directorio de { $launchd } no es un directorio real

journal.cli.host.service.launchd.observation.was.not.utf = la observación de { $launchd } no era UTF-8

journal.cli.host.service.launchd.omitted.or.malformed.arguments = { $launchd } omitió los argumentos o están mal formados

journal.cli.host.service.launchd.omitted.the.runtime.state = { $launchd } omitió el estado del entorno de ejecución

journal.cli.host.service.malformed.systemd.property = propiedad mal formada de { $systemd }

journal.cli.host.service.manager.disable.service = no se pudo desactivar el servicio: { $result }

journal.cli.host.service.manager.disable.stale.service = no se pudo desactivar el servicio desactualizado: { $result }

journal.cli.host.service.manager.enable.service = no se pudo activar el servicio: { $result }

journal.cli.host.service.manager.load.installed.service = no se pudo cargar el servicio instalado: { $result }

journal.cli.host.service.manager.load.service = no se pudo cargar el servicio: { $result }

journal.cli.host.service.manager.load.service.after.changing.journal.folder = no se pudo cargar el servicio después de cambiar la carpeta del diario: { $result }

journal.cli.host.service.manager.reload.service.after.changing.journal.folder = no se pudo recargar el servicio después de cambiar la carpeta del diario: { $result }

journal.cli.host.service.manager.reload.systemd = no se pudo recargar systemd: { $result }

journal.cli.host.service.manager.reload.systemd.for.route.repair = no se pudo recargar systemd para reparar la ruta: { $result }

journal.cli.host.service.manager.request.launchd.unload = no se pudo solicitar la descarga de launchd: { $result }

journal.cli.host.service.manager.restart.service = no se pudo reiniciar el servicio: { $result }

journal.cli.host.service.manager.restart.service.for.route.repair = no se pudo reiniciar el servicio para reparar la ruta: { $result }

journal.cli.host.service.manager.start.after.changing.journal.folder = no se pudo iniciar después de cambiar la carpeta del diario: { $result }

journal.cli.host.service.manager.start.service = no se pudo iniciar el servicio: { $result }

journal.cli.host.service.manager.stop.service = no se pudo detener el servicio: { $result }

journal.cli.host.service.manager.stop.service.before.changing.journal.folder = no se pudo detener el servicio antes de cambiar la carpeta del diario: { $result }

journal.cli.host.service.manager.stop.service.before.restart = no se pudo detener el servicio antes de reiniciar: { $result }

journal.cli.host.service.manager.stop.stale.service = no se pudo detener el servicio desactualizado: { $result }

journal.cli.host.service.manager.unload.stale.launchd.service = no se pudo descargar el servicio desactualizado de launchd: { $result }

journal.cli.host.service.manager.unload.stale.service = no se pudo descargar el servicio desactualizado: { $result }

journal.cli.host.service.metadata = metadatos: { $error }

journal.cli.host.service.open.your.journal = abre tu diario: { $url }

journal.cli.host.service.open.your.journal.solstone.journal.open = abre tu diario: { $open }

journal.cli.host.service.parse.stale.launchd.unit = analizar la unidad desactualizada de { $launchd }: { $error }

journal.cli.host.service.port.invalid = error: puerto no válido '{ $value }'

journal.cli.host.service.preserved.service.unit = unidad de servicio conservada { $path }

journal.cli.host.service.read = lectura: { $error }

journal.cli.host.service.ready.timeout = el servicio no quedó listo en 120 segundos

journal.cli.host.service.recheck.stale.launchd.unit = revisar de nuevo la unidad desactualizada de { $launchd }: { $error }

journal.cli.host.service.registration.a.different.unit.answers.to.this.name = { $operation ->
    [change-journal-folder] se rechazó cambiar la carpeta del diario: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta el cambio de la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [clean-stale-launchd-service] se rechazó limpiar el servicio de launchd desactualizado: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta la limpieza del servicio de launchd desactualizado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [install] se rechazó instalar el servicio: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta la instalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [operate] se rechazó operar el servicio: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta la operación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [start-after-changing-journal-folder] se rechazó iniciar después de cambiar la carpeta del diario: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta el inicio después de cambiar la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [status] se rechazó el estado del servicio: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta el estado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [stop] se rechazó detener el servicio: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta la detención -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [uninstall] se rechazó desinstalar el servicio: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta la desinstalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [up] se rechazó el up del servicio: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta el up -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [verify-service-stop] se rechazó verificar la detención del servicio: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta la verificación de la detención del servicio -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
   *[verify-unload] se rechazó verificar la descarga: otra unidad responde a este nombre. el diario no la sobrescribirá. Si eso es deliberado, termina por tu cuenta la verificación de la descarga -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.launch.agent.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] se rechazó cambiar la carpeta del diario: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta el cambio de la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [clean-stale-launchd-service] se rechazó limpiar el servicio de launchd desactualizado: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta la limpieza del servicio de launchd desactualizado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [install] se rechazó instalar el servicio: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta la instalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [operate] se rechazó operar el servicio: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta la operación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [start-after-changing-journal-folder] se rechazó iniciar después de cambiar la carpeta del diario: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta el inicio después de cambiar la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [status] se rechazó el estado del servicio: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta el estado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [stop] se rechazó detener el servicio: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta la detención -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [uninstall] se rechazó desinstalar el servicio: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta la desinstalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [up] se rechazó el up del servicio: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta el up -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [verify-service-stop] se rechazó verificar la detención del servicio: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta la verificación de la detención del servicio -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
   *[verify-unload] se rechazó verificar la descarga: el agente de lanzamiento no es el que el diario gestiona. el diario no lo sobrescribirá. Si eso es deliberado, termina por tu cuenta la verificación de la descarga -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
}

journal.cli.host.service.registration.the.launch.agent.s.program.is.not.a.journal = { $operation ->
    [change-journal-folder] el servicio se negó a cambiar la carpeta del diario: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el cambio de la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [clean-stale-launchd-service] el servicio se negó a limpiar el servicio launchd desactualizado: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la limpieza del servicio launchd desactualizado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [install] el servicio se negó a instalar: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la instalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [operate] el servicio se negó a operar: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la operación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [start-after-changing-journal-folder] el servicio se negó a iniciar después de cambiar la carpeta del diario: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el inicio después de cambiar la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [status] el servicio se negó a consultar el estado: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la consulta de estado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [stop] el servicio se negó a detener: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la detención -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [uninstall] el servicio se negó a desinstalar: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la desinstalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [up] el servicio se negó a ejecutar up: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú up -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [verify-service-stop] el servicio se negó a verificar la detención del servicio: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la detención del servicio -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
   *[verify-unload] el servicio se negó a verificar la descarga: el programa del agente de lanzamiento no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la descarga -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.carries.a.drop.in.journal.did.not = { $operation ->
    [change-journal-folder] el servicio se negó a cambiar la carpeta del diario: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el cambio de la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [clean-stale-launchd-service] el servicio se negó a limpiar el servicio launchd desactualizado: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la limpieza del servicio launchd desactualizado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [install] el servicio se negó a instalar: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la instalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [operate] el servicio se negó a operar: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la operación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [start-after-changing-journal-folder] el servicio se negó a iniciar después de cambiar la carpeta del diario: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el inicio después de cambiar la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [status] el servicio se negó a consultar el estado: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la consulta de estado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [stop] el servicio se negó a detener: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la detención -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [uninstall] el servicio se negó a desinstalar: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la desinstalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [up] el servicio se negó a ejecutar up: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú up -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [verify-service-stop] el servicio se negó a verificar la detención del servicio: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la detención del servicio -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
   *[verify-unload] el servicio se negó a verificar la descarga: la unidad lleva un drop-in que el diario no escribió. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la descarga -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.file.is.not.the.one.journal.manages = { $operation ->
    [change-journal-folder] el servicio se negó a cambiar la carpeta del diario: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el cambio de la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [clean-stale-launchd-service] el servicio se negó a limpiar el servicio launchd desactualizado: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la limpieza del servicio launchd desactualizado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [install] el servicio se negó a instalar: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la instalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [operate] el servicio se negó a operar: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la operación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [start-after-changing-journal-folder] el servicio se negó a iniciar después de cambiar la carpeta del diario: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el inicio después de cambiar la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [status] el servicio se negó a consultar el estado: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la consulta de estado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [stop] el servicio se negó a detener: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la detención -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [uninstall] el servicio se negó a desinstalar: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la desinstalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [up] el servicio se negó a ejecutar up: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú up -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [verify-service-stop] el servicio se negó a verificar la detención del servicio: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la detención del servicio -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
   *[verify-unload] el servicio se negó a verificar la descarga: el archivo de la unidad no es el que administra el diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la descarga -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.is.in.an.unexpected.state = { $operation ->
    [change-journal-folder] el servicio se negó a cambiar la carpeta del diario: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el cambio de la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [clean-stale-launchd-service] el servicio se negó a limpiar el servicio launchd desactualizado: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la limpieza del servicio launchd desactualizado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [install] el servicio se negó a instalar: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la instalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [operate] el servicio se negó a operar: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la operación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [start-after-changing-journal-folder] el servicio se negó a iniciar después de cambiar la carpeta del diario: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el inicio después de cambiar la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [status] el servicio se negó a consultar el estado: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la consulta de estado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [stop] el servicio se negó a detener: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la detención -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [uninstall] el servicio se negó a desinstalar: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la desinstalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [up] el servicio se negó a ejecutar up: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú up -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [verify-service-stop] el servicio se negó a verificar la detención del servicio: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la detención del servicio -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
   *[verify-unload] el servicio se negó a verificar la descarga: la unidad está en un estado inesperado. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la descarga -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    }

journal.cli.host.service.registration.the.unit.s.execstart.is.not.a.journal.launcher = { $operation ->
    [change-journal-folder] el servicio se negó a cambiar la carpeta del diario: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el cambio de la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [clean-stale-launchd-service] el servicio se negó a limpiar el servicio launchd desactualizado: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la limpieza del servicio launchd desactualizado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [install] el servicio se negó a instalar: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la instalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [operate] el servicio se negó a operar: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la operación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [start-after-changing-journal-folder] el servicio se negó a iniciar después de cambiar la carpeta del diario: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú el inicio después de cambiar la carpeta del diario -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [status] el servicio se negó a consultar el estado: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la consulta de estado -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [stop] el servicio se negó a detener: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la detención -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [uninstall] el servicio se negó a desinstalar: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la desinstalación -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [up] el servicio se negó a ejecutar up: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú up -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    [verify-service-stop] el servicio se negó a verificar la detención del servicio: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la detención del servicio -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
   *[verify-unload] el servicio se negó a verificar la descarga: el ExecStart de la unidad no es un lanzador del diario. el diario no lo sobrescribirá. Si eso es deliberado, termina tú la verificación de la descarga -- en Linux: `{ $restart_command }` -- y luego confirma la compilación en ejecución con `{ $inspect_command }`
    }

journal.cli.host.service.registration.unknown = { $operation ->
    [change-journal-folder] no se pudo revisar el registro del servicio al cambiar la carpeta del diario: { $cause }
    [clean-stale-launchd-service] no se pudo revisar el registro del servicio al limpiar el servicio launchd desactualizado: { $cause }
    [install] no se pudo revisar el registro del servicio al instalar: { $cause }
    [operate] no se pudo revisar el registro del servicio al operar: { $cause }
    [start-after-changing-journal-folder] no se pudo revisar el registro del servicio al iniciar después de cambiar la carpeta del diario: { $cause }
    [status] no se pudo revisar el registro del servicio al consultar el estado: { $cause }
    [stop] no se pudo revisar el registro del servicio al detener: { $cause }
    [uninstall] no se pudo revisar el registro del servicio al desinstalar: { $cause }
    [up] no se pudo revisar el registro del servicio al ejecutar up: { $cause }
    [verify-service-stop] no se pudo revisar el registro del servicio al verificar la detención del servicio: { $cause }
   *[verify-unload] no se pudo revisar el registro del servicio al verificar la descarga: { $cause }
    }

journal.cli.host.service.remove.service.unit = quitar la unidad de servicio: { $error }

journal.cli.host.service.remove.stale.launchd.unit = quitar la unidad desactualizada de { $launchd }: { $error }

journal.cli.host.service.removed = se quitó { $path }

journal.cli.host.service.removed.stale.launchd.plist = se quitó el plist desactualizado de { $launchd } { $path }

journal.cli.host.service.removed.stale.service.registration = se quitó el registro desactualizado del servicio

journal.cli.host.service.route.repair.service.launcher.is.not.an.executable.regular = el lanzador del servicio de reparación de ruta no es un archivo normal ejecutable

journal.cli.host.service.route.repair.service.launcher.is.unavailable = el lanzador del servicio de reparación de ruta no está disponible: { $error }

journal.cli.host.service.route.repair.service.unit.has.an.invalid.command.tail = la unidad del servicio de reparación de ruta tiene una cola de comando no válida

journal.cli.host.service.route.repair.service.unit.has.an.invalid.execstart = la unidad del servicio de reparación de ruta tiene un { $exec_start } no válido

journal.cli.host.service.route.repair.service.unit.has.no.execstart = la unidad del servicio de reparación de ruta no tiene { $exec_start }

journal.cli.host.service.route.repair.service.unit.is.not.utf = la unidad del servicio de reparación de ruta no es UTF-8

journal.cli.host.service.run.solstone.journal.setup.or.solstone.journal.service.install = ejecuta '{ $setup }' o '{ $install }' para instalarlo.

journal.cli.host.service.scan.launchd.directory = examinar el directorio de { $launchd }: { $error }

journal.cli.host.service.scan.launchd.entry = examinar la entrada de { $launchd }: { $error }

journal.cli.host.service.service.became.active.during.the.journal.folder.change = el servicio se activó durante el cambio de carpeta del diario

journal.cli.host.service.service.command.output.exceeded.kib = la salida del comando del servicio superó 256 KiB

journal.cli.host.service.service.command.output.failed = no se pudo obtener la salida del comando del servicio: { $error }

journal.cli.host.service.service.command.spawn.failed = no se pudo crear el proceso del comando del servicio: { $error }

journal.cli.host.service.service.command.timed.out = el comando del servicio agotó el tiempo de espera

journal.cli.host.service.service.command.wait.failed = no se pudo esperar el comando del servicio: { $error }

journal.cli.host.service.service.directory.contains.unsafe.component = el directorio del servicio contiene un componente no seguro

journal.cli.host.service.service.directory.escaped.home = el directorio del servicio salió del directorio personal

journal.cli.host.service.service.enabled = servicio activado

journal.cli.host.service.service.health.path.is.not.a.real.directory = la ruta de estado del servicio no es un directorio real

journal.cli.host.service.service.home = directorio personal del servicio: { $error }

journal.cli.host.service.service.install.could.not.resolve.journal = instalación del servicio: no se pudo resolver el diario

journal.cli.host.service.service.install.current.executable.has.no.directory = instalación del servicio: el ejecutable actual no tiene directorio

journal.cli.host.service.service.install.invalid.installation.identity.arguments = instalación del servicio: argumentos de identidad de instalación no válidos: { $error }

journal.cli.host.service.service.install.requires.installation.identity.arguments = la instalación del servicio requiere argumentos de identidad de instalación

journal.cli.host.service.service.installed = servicio: instalado

journal.cli.host.service.service.journal.root.is.not.a.real.directory = la raíz del diario del servicio no es un directorio real

journal.cli.host.service.service.launcher.is.not.an.executable.regular.file = el lanzador del servicio no es un archivo normal ejecutable

journal.cli.host.service.service.launcher.is.unavailable = el lanzador del servicio no está disponible: { $error }

journal.cli.host.service.service.loaded.into.launchd = servicio cargado en { $launchd }

journal.cli.host.service.service.lock = bloqueo del servicio: { $error }

journal.cli.host.service.service.management.is.supported.only.on.linux.and.mac = la administración del servicio solo es compatible con linux y mac

journal.cli.host.service.service.not.installed = servicio: no instalado

journal.cli.host.service.service.not.installed.run.solstone.journal.service.install.first = el servicio no está instalado. ejecuta '{ $install }' primero.

journal.cli.host.service.service.path.is.not.utf = la ruta del servicio no es UTF-8

journal.cli.host.service.service.registration.has.no.matching.managed.file = el registro del servicio no tiene un archivo administrado que coincida

journal.cli.host.service.service.registration.has.no.valid.port = el registro del servicio no tiene un puerto válido

journal.cli.host.service.service.restarted = se reinició el servicio

journal.cli.host.service.service.runtime.state.was.unexpected = { $operation ->
    [change-journal-folder] al cambiar la carpeta del diario, el estado del entorno de ejecución del servicio fue inesperado
    [clean-stale-launchd-service] al limpiar el servicio launchd desactualizado, el estado del entorno de ejecución del servicio fue inesperado
    [install] al instalar, el estado del entorno de ejecución del servicio fue inesperado
    [operate] al operar, el estado del entorno de ejecución del servicio fue inesperado
    [start-after-changing-journal-folder] al iniciar después de cambiar la carpeta del diario, el estado del entorno de ejecución del servicio fue inesperado
    [status] al consultar el estado, el estado del entorno de ejecución del servicio fue inesperado
    [stop] al detener, el estado del entorno de ejecución del servicio fue inesperado
    [uninstall] al desinstalar, el estado del entorno de ejecución del servicio fue inesperado
    [up] al ejecutar up, el estado del entorno de ejecución del servicio fue inesperado
    [verify-service-stop] al verificar la detención del servicio, el estado del entorno de ejecución del servicio fue inesperado
   *[verify-unload] al verificar la descarga, el estado del entorno de ejecución del servicio fue inesperado
    }

journal.cli.host.service.service.started = se inició el servicio

journal.cli.host.service.service.stderr.reader.panicked = el lector de stderr del servicio hizo panic

journal.cli.host.service.service.stderr.unavailable = stderr del servicio no está disponible

journal.cli.host.service.service.stdout.reader.panicked = el lector de stdout del servicio hizo panic

journal.cli.host.service.service.stdout.unavailable = stdout del servicio no está disponible

journal.cli.host.service.service.stopped = se detuvo el servicio

journal.cli.host.service.service.stopped.but.its.ready.state.could.not.be = el servicio se detuvo, pero no se pudo borrar su estado de listo: { $error }

journal.cli.host.service.service.unit.changed.before.publication.nothing.was.written = la unidad de servicio cambió antes de la publicación; no se escribió nada

journal.cli.host.service.service.unit.changed.during.operation.preserved.replacement = la unidad de servicio cambió durante la operación; se conservó el reemplazo

journal.cli.host.service.service.unit.has.no.parent = la unidad de servicio no tiene directorio padre

journal.cli.host.service.service.unit.has.no.trusted.home.ancestor = la unidad de servicio no tiene un ancestro de confianza del directorio personal

journal.cli.host.service.service.unit.publication.failed = no se pudo publicar la unidad de servicio: { $error }

journal.cli.host.service.service.unit.published.with.uncertain.durability = la unidad de servicio se publicó con una durabilidad incierta: { $other }

journal.cli.host.service.service.was.not.installed = el servicio no estaba instalado

journal.cli.host.service.sign.in.invalid = error: { $flag } espera { $literal } o { $literal_3 }

journal.cli.host.service.stale.launchd.plist.changed.during.cleanup.preserved = el plist desactualizado de { $launchd } cambió durante la limpieza; se conservó { $path }

journal.cli.host.service.stale.launchd.unit.has.a.foreign.label = la unidad desactualizada de { $launchd } tiene una etiqueta ajena

journal.cli.host.service.stale.launchd.unit.has.conflicting.program.fields = la unidad desactualizada de { $launchd } tiene campos de programa en conflicto

journal.cli.host.service.stale.launchd.unit.has.no.label = la unidad desactualizada de { $launchd } no tiene etiqueta

journal.cli.host.service.stale.launchd.unit.has.no.launcher = la unidad desactualizada de { $launchd } no tiene lanzador

journal.cli.host.service.stale.launchd.unit.is.foreign.or.unrecognized = la unidad desactualizada de { $launchd } es ajena o no se reconoce

journal.cli.host.service.stale.launchd.unit.is.not.a.dictionary = la unidad desactualizada de { $launchd } no es un diccionario

journal.cli.host.service.state.running = estado: en ejecución ({ $manager })

journal.cli.host.service.state.stopped = estado: detenido

journal.cli.host.service.subcommand.unknown = subcomando desconocido: { $value }; disponibles: { $literal }

journal.cli.host.service.systemd.observation.was.not.utf = la observación de { $systemd } no era UTF-8

journal.cli.host.service.the.previous.service.did.not.stop.before.replacement = el servicio anterior no se detuvo antes del reemplazo

journal.cli.host.service.the.solstone.executable.has.no.containing.directory = el ejecutable de { $solstone } no tiene un directorio contenedor

journal.cli.host.service.unit.exceeds.mib = la unidad supera 1 MiB

journal.cli.host.service.unit.path.has.no.trusted.home.ancestor = la ruta de la unidad no tiene un ancestro de confianza del directorio personal

journal.cli.host.service.unrecognized.systemd.active.substate.pair = par activo/subestado de { $systemd } no reconocido

journal.cli.host.service.unsafe.service.directory = directorio de servicio no seguro: { $path }

journal.cli.host.service.usage = Uso: { $syntax }{ $line_break }{ $spaces_7 }{ $syntax_4 }{ $spaces_2 }(predeterminado: 5015; solo { $term }){ $line_break }{ $spaces_7 }{ $syntax_7 }{ $spaces_2 }(reinicio; { $flag } no hace nada si no está instalado){ $line_break }{ $spaces_7 }{ $syntax_9 }{ $spaces_29 }(inicio + estado; el servicio debe estar instalado){ $line_break }{ $spaces_7 }{ $syntax_11 }{ $spaces_27 }(detención){ $line_break }

journal.cli.host.service.windows.only = este comando de servicio solo está disponible en Windows

journal.cli.host.service.wrote = se escribió { $path }

journal.cli.host.settings.bind = convey{ $line_break }{ $space_2 }enlace:{ $space_14 }{ $address }

journal.cli.host.settings.convey.error.argument.convey.command.invalid.choice.choose.from.status = { $command }: error: argumento { $literal }: opción no válida: '{ $value }' (elige entre { $literal_3 })

journal.cli.host.settings.convey.help = uso: { $syntax }{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_4 }Muestra el estado del enlace de convey y de la { $term } del panel{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.settings.convey.usage = uso: { $syntax }{ $line_break }

journal.cli.host.settings.error.argument.section.invalid.choice.choose.from.convey = { $command }: error: argumento { $literal }: opción no válida: '{ $value }' (elige entre { $literal_3 })

journal.cli.host.settings.error.unrecognized.arguments = { $command }: error: argumentos no reconocidos: { $value }

journal.cli.host.settings.help = uso: { $syntax }{ $line_break }{ $line_break }Administra la configuración local del diario{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_7 }Administra la configuración de convey{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_2 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_11 }{ $spaces_4 }Activa el registro de depuración{ $line_break }

journal.cli.host.settings.open = ábrelo con:{ $space_6 }{ $command }

journal.cli.host.settings.port.read = Error: { $path }: { $detail }

journal.cli.host.settings.status.help = uso: { $syntax }{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_5 }{ $spaces_6 }Imprime el estado legible por máquina.{ $line_break }

journal.cli.host.settings.url = url del panel:{ $space_5 }{ $url }

journal.cli.host.settings.usage = uso: { $syntax }{ $line_break }

journal.cli.host.setup.error = { $literal }: error: { $detail }

journal.cli.host.setup.usage = uso: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $spaces_21 }{ $syntax_8 }{ $line_break }{ $spaces_21 }{ $syntax_9 } (solo { $term }){ $line_break }

journal.cli.host.spl.help = uso: { $syntax }{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }Activa el registro de depuración{ $line_break }

journal.cli.host.spl.service.spl.service.failed = el servicio spl falló: { $class }

journal.cli.host.spl.usage = uso: { $syntax }{ $line_break }

journal.cli.host.start.help = uso: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }{ $line_break }Supervisa el estado del sistema del diario{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }{ $term } puerto (0 = seleccionar automáticamente un puerto disponible){ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Desactiva la ejecución de procesamiento diario a medianoche{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }No inicies el servidor { $term_17 } (ejecútalo a mano para{ $line_break }{ $spaces_21 }depurarlo){ $line_break }{ $spaces_2 }{ $option_18 }{ $spaces_11 }No inicies el servidor de retransmisión de la red privada{ $line_break }{ $spaces_2 }{ $option_20 }{ $spaces_8 }No inicies la aplicación web { $term }{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_6 }No inicialices ni ejecutes el motor de programación{ $line_break }{ $spaces_2 }{ $option_23 }{ $spaces_4 }Exige que el proceso padre directo siga activo{ $line_break }{ $spaces_2 }{ $option_25 }{ $spaces_2 }Usa esta ruta como raíz del diario en lugar de la{ $line_break }{ $spaces_21 }resolución normal del diario.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Puerto de puerta del dispositivo vinculado (predeterminado: 7657){ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_6 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_29 }{ $spaces_8 }Activa el registro de depuración{ $line_break }

journal.cli.host.start.usage = uso: { $syntax }{ $line_break }{ $spaces_21 }{ $syntax_4 }{ $line_break }{ $spaces_21 }{ $syntax_5 }{ $line_break }{ $spaces_21 }{ $syntax_6 }{ $line_break }{ $spaces_21 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.admission.wait.active = otro inicio de { $brand } está en espera tras encontrar actividad reciente del diario.{ $break_line }este inicio no continuó mientras esa espera seguía activa.{ $break_line }espera un momento y vuelve a intentarlo.

journal.cli.host.supervisor.admission.wait.terminal = hay un latido reciente de otra ejecución.{ $break_line }la app de solstone no inició mientras ese latido seguía presente.{ $break_line }espera un momento y vuelve a intentarlo.

journal.cli.host.supervisor.admission.wait.unverifiable = no se pudo verificar el estado de arranque.{ $break_line }espera un momento y vuelve a intentarlo.

journal.cli.host.supervisor.already.running = tu diario ya está en ejecución.{ $break_line }{ $break_line }este inicio se apartó para no alterar el que está en ejecución. no hay nada que hacer.{ $break_line }

journal.cli.host.supervisor.binding.home = directorio personal: { $error }

journal.cli.host.supervisor.binding.installation.root = raíz de la instalación: { $error }

journal.cli.host.supervisor.binding.journal.token = token del diario: { $error }

journal.cli.host.supervisor.binding.owner.storage = almacenamiento del propietario: { $error }

journal.cli.host.supervisor.binding.root.token = token raíz: { $error }

journal.cli.host.supervisor.binding.saved.binding = vinculación guardada: { $error }

journal.cli.host.supervisor.binding.unreadable = no se pudo cargar la vinculación de instalación guardada

journal.cli.host.supervisor.help = uso: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }{ $line_break }Supervisa el estado del sistema del diario{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $subcommand }{ $spaces_15 }{ $term } puerto (0 = seleccionar automáticamente un puerto disponible){ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_9 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_14 }{ $spaces_9 }Desactiva la ejecución de procesamiento diario a medianoche{ $line_break }{ $spaces_2 }{ $option_15 }{ $spaces_8 }No inicies el servidor { $term_17 } (ejecútalo a mano para{ $line_break }{ $spaces_21 }depurarlo){ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_11 }No inicies el servidor de retransmisión de la red privada{ $line_break }{ $spaces_2 }{ $option_21 }{ $spaces_8 }No inicies la aplicación web { $term }{ $line_break }{ $spaces_2 }{ $option_22 }{ $spaces_6 }No inicialices ni ejecutes el motor de programación{ $line_break }{ $spaces_2 }{ $option_24 }{ $spaces_2 }Usa esta ruta como raíz del diario en lugar de la{ $line_break }{ $spaces_21 }resolución normal del diario.{ $line_break }{ $spaces_2 }{ $flag } { $argument }{ $line_break }{ $spaces_21 }Puerto de puerta del dispositivo vinculado (predeterminado: 7657){ $line_break }{ $spaces_2 }{ $option_27 }{ $spaces_6 }Activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_28 }{ $spaces_8 }Activa el registro de depuración{ $line_break }

journal.cli.host.supervisor.journal.program.folder = Reinstalar o desinstalar quita esta carpeta del programa. Elige una ubicación del diario fuera de ella. Si tu diario ya está ahí, cierra la app del diario. Si la configuración inicial terminó, ejecuta `{ $stop }`, luego `{ $uninstall }` y espera a que tenga éxito. Mueve toda la carpeta del diario fuera de la carpeta del programa antes de reinstalar o desinstalar, y luego selecciona su nueva ubicación con `{ $setup }`, reemplazando { $path } por su nueva ubicación.

journal.cli.host.supervisor.journal.token.mismatch = no se pudo verificar esta instalación.{ $line_break }ejecuta `{ $setup }` para revisarla. si la configuración inicial termina con éxito, vuelve a intentarlo.{ $line_break }detalles: la vinculación de instalación guardada es para otro diario

journal.cli.host.supervisor.lifecycle.redirect = { $supervisor } es el comando de arranque del servidor (toma un puerto). Para el ciclo de vida, usa: { $service_verb }. ¿Quisiste decir: { $requested } ?

journal.cli.host.supervisor.recovery.closing = este inicio no pudo continuar.{ $line_break }{ $line_break }la última ejecución todavía se está cerrando. espera un momento y vuelve a intentarlo.{ $line_break }{ $line_break }tus recuerdos no se tocaron.{ $line_break }

journal.cli.host.supervisor.recovery.coordinator.retirement.unverified = detalles: no se pudo confirmar que un proceso auxiliar se detuvo

journal.cli.host.supervisor.recovery.coordinator.stopped.before.ready = detalles: un proceso auxiliar se detuvo antes de estar listo

journal.cli.host.supervisor.recovery.identity.establishment = detalles: no se pudo identificar un proceso auxiliar

journal.cli.host.supervisor.recovery.initial.admission.handshake = detalles: un proceso auxiliar no respondió a tiempo

journal.cli.host.supervisor.recovery.launch = detalles: no se pudo iniciar un proceso auxiliar

journal.cli.host.supervisor.recovery.retry = este inicio no pudo continuar.{ $line_break }{ $line_break }espera un momento y vuelve a intentarlo.{ $line_break }{ $line_break }tus recuerdos no se tocaron.{ $line_break }

journal.cli.host.supervisor.supervisor.runtime.unavailable = el entorno de ejecución del supervisor no está disponible: { $error }

journal.cli.host.supervisor.supervisor.service.capture.unavailable = la captura del servicio del supervisor no está disponible: { $error }

journal.cli.host.supervisor.usage = uso: { $syntax }{ $line_break }{ $spaces_26 }{ $syntax_4 }{ $line_break }{ $spaces_26 }{ $syntax_5 }{ $line_break }{ $spaces_26 }{ $syntax_6 }{ $line_break }{ $spaces_26 }{ $syntax_7 }{ $line_break }

journal.cli.host.supervisor.your.journal.can.t.start.because.another.account.on.this = tu diario no puede iniciar porque otra cuenta de esta computadora está ejecutando su diario. en una computadora se ejecuta un solo diario a la vez, así que el tuyo puede iniciar cuando esa cuenta detenga su diario o cierre sesión.

journal.cli.host.supervisor.your.journal.can.t.start.because.port.on.this.computer = tu diario no puede iniciar porque el puerto { $port } de esta computadora ya está en uso, por lo general por un diario en ejecución en otra cuenta. solo un diario puede ejecutarse en esta computadora a la vez.

journal.cli.host.thinking.chatgpt.help = uso: { $syntax }{ $line_break }{ $line_break }Administra el inicio de sesión, los tokens y el catálogo de modelos de { $term }.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_14 }Inicia sesión en { $term } con { $term_9 }{ $line_break }{ $spaces_4 }{ $subcommand_10 }{ $spaces_13 }Cierra sesión y revoca las credenciales activas de { $term }{ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_15 }Muestra el estado de autenticación actual de { $term }{ $line_break }{ $spaces_4 }{ $subcommand_14 }{ $spaces_15 }Lista los modelos disponibles de { $term }{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.thinking.chatgpt.models.help = uso: { $syntax }{ $line_break }{ $line_break }Lista los modelos disponibles de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Raíz del diario{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Emite { $term_11 } legible por máquina{ $line_break }

journal.cli.host.thinking.chatgpt.models.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.help = uso: { $syntax }{ $line_break }{ $line_break }Inicia sesión en { $term } con { $term_4 } en el navegador.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_8 }{ $spaces_8 }Raíz del diario{ $line_break }{ $spaces_2 }{ $option_10 }{ $spaces_10 }No abras el navegador predeterminado de forma automática{ $line_break }

journal.cli.host.thinking.chatgpt.sign.in.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.help = uso: { $syntax }{ $line_break }{ $line_break }Cierra sesión y revoca las credenciales activas de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Raíz del diario{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_14 }Quita el archivo de credenciales guardado y el registro{ $line_break }

journal.cli.host.thinking.chatgpt.sign.out.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.status.help = uso: { $syntax }{ $line_break }{ $line_break }Muestra el estado de autenticación actual de { $term }.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_8 }Raíz del diario{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_16 }Emite { $term_11 } legible por máquina{ $line_break }

journal.cli.host.thinking.chatgpt.status.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.chatgpt.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.credential.chatgpt.auth.server.temporarily.unavailable = el servidor de autenticación de { $literal_1 } no está disponible temporalmente (429 o 5xx)

journal.cli.host.thinking.credential.chatgpt.credential.store.busy = el almacén de credenciales de { $literal_1 } está ocupado

journal.cli.host.thinking.credential.chatgpt.request.was.by.auth = el servidor de autenticación rechazó la solicitud de { $literal_1 }

journal.cli.host.thinking.credential.chatgpt.request.was.code = se rechazó la solicitud de { $literal_1 }: { $code }

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload = el servidor de { $literal_1 } devolvió una carga útil malformada ({ $field } falta o el valor no es válido)

journal.cli.host.thinking.credential.chatgpt.server.returned.malformed.payload.server.returned.malformed.payload = el servidor de { $literal_1 } devolvió una carga útil malformada

journal.cli.host.thinking.credential.chatgpt.token.grant.was.received = se recibió la concesión del token de { $literal_1 } desde el servidor, pero no se pudo guardar en el disco

journal.cli.host.thinking.credential.i.o.during.credential.operation = Error de E/S durante la operación de credenciales: { $msg }

journal.cli.host.thinking.credential.network.communicating.with.chatgpt.auth = error de red al comunicarse con los servidores de autenticación de { $literal_1 }

journal.cli.host.thinking.credential.path.credential.storage.unreadable.or = { $path }: el almacenamiento de credenciales no se puede leer o está dañado; ejecuta '{ $literal_1 }'

journal.cli.host.thinking.credential.this.chatgpt.account.isn.t = esta cuenta de { $literal_1 } no es elegible

journal.cli.host.thinking.help = uso: { $syntax }{ $line_break }{ $line_break }Elige el carril de razonamiento del diario o administra la autenticación de { $term }.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_4 }{ $subcommand }{ $spaces_13 }Establece el carril de razonamiento ({ $literal }, { $literal_10 } o { $literal_11 }){ $line_break }{ $spaces_4 }{ $subcommand_12 }{ $spaces_14 }Administra el inicio de sesión y los tokens de { $term }{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.thinking.run.chatgpt.sign.in.account.mismatch.run.thinking.chatgpt = la cuenta no coincide: ejecuta '{ $literal_1 }'

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.failed.outcome = no se pudo iniciar sesión en { $literal_1 }: { $outcome }

journal.cli.host.thinking.run.chatgpt.sign.in.chatgpt.sign.in.required.run = hace falta iniciar sesión en { $literal_2 }: ejecuta '{ $literal_1 }'

journal.cli.host.thinking.run.chatgpt.sign.in.signed.in.chatgpt.successfully = iniciaste sesión en { $literal_1 } correctamente.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt = cerraste sesión de { $literal_1 }.

journal.cli.host.thinking.run.chatgpt.sign.out.signed.out.chatgpt.complete.sign = cerraste sesión de { $literal_1 }.{ $line_break }Para completar el cierre de sesión, desconecta solstone en la configuración de tu cuenta de { $literal_1 }.

journal.cli.host.thinking.run.chatgpt.status.plan.usage.declined = uso del plan: rechazado

journal.cli.host.thinking.run.chatgpt.status.status.not.signed.in = estado: sin sesión iniciada

journal.cli.host.thinking.run.chatgpt.status.status.signed.in = estado: con sesión iniciada

journal.cli.host.thinking.set.lane.help = uso: { $syntax }{ $line_break }{ $line_break }Establece el carril de razonamiento que usa { $literal }.{ $line_break }{ $line_break }argumentos posicionales:{ $line_break }{ $spaces_2 }{ $choices }{ $line_break }{ $spaces_24 }Carril de razonamiento que se activa.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_12 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_9 }{ $spaces_3 }{ $argument } proveedor: { $literal_12 }, { $literal_13 }, { $literal_14 }, { $literal_15 } o { $literal_16 }{ $line_break }{ $spaces_2 }{ $option_17 }{ $spaces_9 }Nombre del modelo { $argument } en la nube{ $line_break }{ $spaces_2 }{ $option_19 }{ $spaces_8 }Raíz del diario{ $line_break }

journal.cli.host.thinking.set.lane.usage = uso: { $syntax }{ $line_break }

journal.cli.host.thinking.usage = uso: { $syntax }{ $line_break }

journal.cli.host.top.failure = { $command_solstone_journal } top: { $error }

journal.cli.host.top.help = uso: { $syntax }{ $line_break }{ $line_break }Muestra la actividad interactiva del servicio, de la observación, de las tareas y del cerebro.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_5 }muestra este mensaje de ayuda y sale{ $line_break }{ $spaces_2 }{ $option_6 }{ $spaces_2 }activa la salida detallada{ $line_break }{ $spaces_2 }{ $option_7 }{ $spaces_4 }activa la salida de depuración{ $line_break }

journal.cli.host.top.usage = uso: { $syntax }{ $line_break }

journal.cli.host.up.help = uso: { $syntax }{ $line_break }{ $line_break }Inicia el servicio del diario instalado si no está en ejecución y luego espera hasta que esté listo.{ $line_break }{ $line_break }opciones:{ $line_break }{ $spaces_2 }{ $option }{ $spaces_2 }muestra este mensaje de ayuda y sale{ $line_break }

journal.cli.host.usage.commands = Uso:{ $line_break }{ $commands }

journal.cli.host.warm.development = Un GAP es un hermano del diseño de desarrollo que no se compiló y por eso no se ejecutó; se informa por nombre, pero no hace fallar a warm.

journal.cli.host.warm.inventory = Warm revisa los { $count } binarios nativos declarados por las hojas de empaquetado maturin { $bindings } del host.

journal.cli.host.warm.linux.dynamic = En Linux, { $solstone_core_speakers_analyze } y { $solstone_core_vad_analyze } cargan de forma dinámica el { $onnx } incluido, y { $solstone_core_vulkan_probe } carga de forma dinámica el cargador de { $vulkan } del equipo. Warm demuestra la resolución de sus cargadores y también su arranque. { $solstone_core_describe } no se afirma aquí como estático ni como dinámico; la validación del wheel solo prohíbe un { $ffmpeg } enlazado de forma dinámica.

journal.cli.host.warm.linux.static = En Linux, { $solstone_core }, { $solstone_core_journal }, { $solstone_retention }, { $solstone_core_sol } y { $solstone_core_depict } están enlazados de forma estática con musl. Warm demuestra que arrancan y llegan a su propio código; no hay una resolución de cargador dinámico que demostrar.

journal.cli.host.warm.mac = En macOS, todo Mach-O enlaza { $library }. La propiedad no está vacía, pero difiere de Linux: ahí warm expone fallos de firma, de cuarentena y de { $dyld }.

journal.cli.host.warm.platforms = Filas de warm restringidas por plataforma: { $rows }. Informan NO APLICA cuando su aplicabilidad no incluye este equipo y no hacen fallar a warm.

journal.cli.host.warm.repair.development = { $space_2 }reparación: Compila este crate en un entorno de desarrollo configurado.

journal.cli.host.warm.repair.execution = { $space_2 }reparación: Repara el fallo de ejecución del host que se informó y luego reinstala la herramienta del host propietaria si hace falta.

journal.cli.host.warm.repair.missing = { $space_2 }reparación: Reinstala o actualiza la herramienta del host propietaria con la familia de instaladores que creó este entorno.

journal.cli.host.warm.repair.platform = { $space_2 }reparación: No hace falta reparar en esta plataforma.

journal.cli.host.warm.row.development = GAP { $binary }: { $path } no está junto al ejecutable de desarrollo

journal.cli.host.warm.row.executable = NO SE PUEDE CARGAR { $binary }: no se puede resolver el ejecutable actual: { $error }

journal.cli.host.warm.row.exit = SE EJECUTÓ { $binary }: devolvió la salida { $code }

journal.cli.host.warm.row.library = NO SE PUEDE CARGAR { $binary }: no se pudo cargar la biblioteca compartida { $library }

journal.cli.host.warm.row.missing = AUSENTE { $binary }: { $path } no está junto al ejecutable en ejecución

journal.cli.host.warm.row.no.code = NO SE PUEDE CARGAR { $binary }: terminó sin un código de salida

journal.cli.host.warm.row.platform = NO APLICA { $binary }: el binario solo para Linux no aplica en esta plataforma

journal.cli.host.warm.row.signal = NO SE PUEDE CARGAR { $binary }: terminó por la señal { $signal }

journal.cli.host.warm.row.spawn = NO SE PUEDE CARGAR { $binary }: no se pudo crear el proceso { $path }: { $error }

journal.cli.host.warm.summary = { $quantity ->
    [one] Resumen: { $ran } se ejecutaron, { $missing } ausentes, { $cannot_load } no se pueden cargar, { $gaps } gap, { $not_applicable } no aplican.
    [many] Resumen: { $ran } se ejecutaron, { $missing } ausentes, { $cannot_load } no se pueden cargar, { $gaps } gaps, { $not_applicable } no aplican.
   *[other] Resumen: { $ran } se ejecutaron, { $missing } ausentes, { $cannot_load } no se pueden cargar, { $gaps } gaps, { $not_applicable } no aplican.
    }

journal.cli.host.warm.unresolved = { $space_2 }biblioteca sin resolver: { $library }

journal.cli.host.windows.service.adopted.installation.has.no.journal.path = la instalación adoptada no tiene ruta del diario

journal.cli.host.windows.service.background.service.is.not.installed = el servicio en segundo plano no está instalado

journal.cli.host.windows.service.background.service.stopped = el servicio en segundo plano se detuvo

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.changed.run = no se pudo cambiar el soporte en segundo plano de tu diario.{ $break_line }ejecuta `{ $command_solstone_journal_service_status }` para revisarlo.{ $break_line }detalles: { $reason }

journal.cli.host.windows.service.background.support.for.your.journal.couldn.t.be.checked.try = no se pudo revisar el soporte en segundo plano de tu diario.{ $break_line }vuelve a intentarlo en un momento. si sigue fallando, incluye los detalles de abajo en una solicitud de soporte.{ $break_line }detalles: { $reason }

journal.cli.host.windows.service.before.uninstall.deadline.is.out.of.range = el plazo de antes de desinstalar está fuera de rango

journal.cli.host.windows.service.cannot.identify.one.running.service.task = no se puede identificar una sola tarea de servicio en ejecución

journal.cli.host.windows.service.cannot.resolve.task.artifact.directory = no se puede resolver el directorio de artefactos de la tarea

journal.cli.host.windows.service.clean.uninstall.requires.an.adopted.installation.identity = la desinstalación limpia requiere una identidad de instalación adoptada

journal.cli.host.windows.service.could.not.clear.prior.update.intent = no se pudo borrar la intención de actualización previa: { $error }

journal.cli.host.windows.service.could.not.clear.windows.sign.in.resume = no se pudo borrar la reanudación del inicio de sesión de { $literal_windows }: { $error }

journal.cli.host.windows.service.could.not.consume.update.intent = no se pudo consumir la intención de actualización: { $error }

journal.cli.host.windows.service.could.not.inspect.current.executable = no se pudo inspeccionar el ejecutable actual: { $error }

journal.cli.host.windows.service.could.not.load.installation.binding = no se pudo cargar la vinculación de instalación: { $error }

journal.cli.host.windows.service.could.not.locate.owner.base = no se pudo ubicar la base del propietario: { $error }

journal.cli.host.windows.service.could.not.prepare.windows.sign.in.resume = no se pudo preparar la reanudación del inicio de sesión de { $literal_windows }: { $error }

journal.cli.host.windows.service.could.not.read.update.intent.record = no se pudo leer el registro de intención de actualización: { $error }

journal.cli.host.windows.service.could.not.resolve.identity.root.from.executable.directory = no se pudo resolver la raíz de identidad desde el directorio del ejecutable

journal.cli.host.windows.service.could.not.resolve.root.token = no se pudo resolver el token raíz: { $error }

journal.cli.host.windows.service.could.not.resolve.uninstall.root = no se pudo resolver la raíz de desinstalación

journal.cli.host.windows.service.could.not.retrieve.user.sid = no se pudo obtener el { $literal_sid } del usuario: { $error }

journal.cli.host.windows.service.could.not.write.update.intent.record = no se pudo escribir el registro de intención de actualización: { $error }

journal.cli.host.windows.service.direct.worker.stop.request.direct.worker.observation = solicitud de detención del worker directo: { $termination }; observación del worker directo: { $observed }

journal.cli.host.windows.service.error.invalid.port = error: puerto no válido '{ $port }'

journal.cli.host.windows.service.installation.identity.census.is.unreadable = el censo de identidad de la instalación es ilegible

journal.cli.host.windows.service.installation.provider.location.is.unavailable = la ubicación del proveedor de la instalación no está disponible

journal.cli.host.windows.service.installed.command.contains.invalid.text = el comando instalado contiene texto no válido

journal.cli.host.windows.service.installed.task.action.did.not.match.its.readback = la acción de la tarea instalada no coincidió con su relectura

journal.cli.host.windows.service.is.registered.expected = { $registered } está en el registro, se esperaba { $expected }

journal.cli.host.windows.service.it.belongs.to.a.different.installation.of.the.journal = pertenece a otra instalación del diario

journal.cli.host.windows.service.it.is.registered.to.another.account = el registro corresponde a otra cuenta

journal.cli.host.windows.service.it.runs.the.journal.at.expected = ejecuta el diario en { $registered }, se esperaba { $journal }

journal.cli.host.windows.service.journal = diario: { $error }

journal.cli.host.windows.service.journal.path.contains.invalid.text = la ruta del diario contiene texto no válido

journal.cli.host.windows.service.native.process.birth.changed = cambió el nacimiento del proceso nativo

journal.cli.host.windows.service.native.process.birth.is.missing = falta el nacimiento del proceso nativo

journal.cli.host.windows.service.native.process.pid.is.invalid = el PID del proceso nativo no es válido

journal.cli.host.windows.service.native.windows.directory.is.unavailable = el directorio nativo de { $literal_windows } no está disponible

journal.cli.host.windows.service.no.reason = windows no dio un motivo.

journal.cli.host.windows.service.scheduler.returned.no.task.run.instance = el planificador no devolvió una instancia de ejecución de la tarea

journal.cli.host.windows.service.scheduler.rpc.may.have.changed.state.re.inspection.required = { $reason }; { $observation }; el RPC del planificador puede haber cambiado el estado; hace falta revisar de nuevo

journal.cli.host.windows.service.selected.journal.differs.from.the.saved.installation.binding = el diario seleccionado difiere de la vinculación de instalación guardada

journal.cli.host.windows.service.service.exited.abnormally.cleanup.remains.unverified = el servicio salió de forma anómala; la limpieza sigue sin verificarse

journal.cli.host.windows.service.service.forwarder.admission.is.invalid = la admisión del reenviador del servicio no es válida

journal.cli.host.windows.service.service.installation.guard.differs.from.the.saved.binding = la guarda de instalación del servicio difiere de la vinculación guardada

journal.cli.host.windows.service.service.readiness.has.no.exact.native.identity = la preparación del servicio no tiene una identidad nativa exacta

journal.cli.host.windows.service.service.readiness.has.no.installed.forwarder.admission = la preparación del servicio no tiene la admisión del reenviador instalado

journal.cli.host.windows.service.service.readiness.identity.changed = cambió la identidad de preparación del servicio

journal.cli.host.windows.service.service.run.exited.during.admission = la ejecución del servicio salió durante la admisión

journal.cli.host.windows.service.service.scheduler.instance.does.not.match.its.admitted.forwarder = la instancia del planificador del servicio no coincide con su reenviador admitido

journal.cli.host.windows.service.service.stop.deadline.elapsed.cleanup.remains.unverified = se agotó el plazo de detención del servicio; la limpieza sigue sin verificarse

journal.cli.host.windows.service.service.task.changed.during.run.admission = la tarea del servicio cambió durante la admisión de la ejecución

journal.cli.host.windows.service.service.task.changed.during.start = la tarea del servicio cambió durante el inicio

journal.cli.host.windows.service.service.task.changed.during.stop = la tarea del servicio cambió durante la detención

journal.cli.host.windows.service.service.task.changed.while.recording.its.state = la tarea del servicio cambió mientras registraba su estado

journal.cli.host.windows.service.service.task.deletion.was.not.observed = no se observó la eliminación de la tarea del servicio

journal.cli.host.windows.service.service.task.differs.from.the.current.installation.binding = la tarea del servicio difiere de la vinculación de la instalación actual

journal.cli.host.windows.service.service.task.instance.changed.during.stop = la instancia de la tarea del servicio cambió durante la detención

journal.cli.host.windows.service.service.task.instance.changed.while.retaining.native.handles = la instancia de la tarea del servicio cambió mientras conservaba los handles nativos

journal.cli.host.windows.service.service.task.is.absent.and.its.forwarder.cleanup.cannot.be = la tarea del servicio está ausente y no se puede verificar la limpieza de su reenviador

journal.cli.host.windows.service.service.task.is.not.installed = la tarea del servicio no está instalada

journal.cli.host.windows.service.stopped.background.support.for.your.journal.starts.again.when.you = Detenido: el soporte en segundo plano de tu diario vuelve a iniciar la próxima vez que inicies sesión.

journal.cli.host.windows.service.stopped.background.support.for.your.journal.stays.off.until.you = Detenido: el soporte en segundo plano de tu diario sigue desactivado hasta que lo inicies o abras la app del diario.

journal.cli.host.windows.service.supervisor.readiness.not.ready = Preparación del supervisor: no está lista

journal.cli.host.windows.service.supervisor.readiness.ready = Preparación del supervisor: lista

journal.cli.host.windows.service.systemroot.is.unavailable = SystemRoot no está disponible

journal.cli.host.windows.service.task = Tarea: { $task }

journal.cli.host.windows.service.task.control.deadline.elapsed.before.spawn = el plazo de control de la tarea se agotó antes de crear el proceso

journal.cli.host.windows.service.task.control.exchange.failed = falló el intercambio de control de la tarea: { $error }

journal.cli.host.windows.service.task.control.exchange.failed.worker.channel.closed = falló el intercambio de control de la tarea: se cerró el canal del trabajador

journal.cli.host.windows.service.task.control.exchange.timed.out = se agotó el tiempo de espera del intercambio de control de la tarea

journal.cli.host.windows.service.task.control.input.exceeds.its.byte.limit = la entrada de control de la tarea supera su límite de bytes

journal.cli.host.windows.service.task.control.output.exceeds.its.byte.limit = la salida de control de la tarea supera su límite de bytes

journal.cli.host.windows.service.task.control.spawn.failed = falló la creación del proceso de control de la tarea: { $error }

journal.cli.host.windows.service.task.control.stdio.missing = falta el stdio del control de la tarea

journal.cli.host.windows.service.task.control.worker.exited = el trabajador de control de la tarea salió

journal.cli.host.windows.service.task.operation.returned.inconsistent.evidence = la operación de la tarea devolvió evidencia inconsistente

journal.cli.host.windows.service.task.operation.returned.invalid.json = la operación de la tarea devolvió un { $literal_json } no válido

journal.cli.host.windows.service.task.request.is.not.a.json.object = la solicitud de la tarea no es un objeto { $literal_json }

journal.cli.host.windows.service.task.xml.readback.missing = falta la relectura XML de la tarea

journal.cli.host.windows.service.the.installed.solstone.exe.facade.is.unavailable = la fachada solstone.exe instalada no está disponible

journal.cli.host.windows.service.the.registered.task.has.no.readable.definition = la tarea registrada no tiene una definición legible

journal.cli.host.windows.service.update.intent.record.is.invalid = el registro de intención de actualización no es válido

journal.cli.host.windows.service.windows.run.key.was.not.opened = no se abrió la clave Run de { $literal_windows }

journal.cli.host.windows.service.windows.service.action.contains.non.unicode.text = la acción del servicio de windows contiene texto que no es Unicode

journal.cli.host.windows.service.windows.service.action.differs.from.the.complete.saved.binding = la acción del servicio de windows difiere del enlace guardado completo

journal.cli.host.windows.service.windows.service.binding.unavailable = el enlace del servicio de windows no está disponible

journal.cli.host.windows.service.windows.sign.in.resume.command.exceeds.the.run.key.limit = el comando de reanudación del inicio de sesión de { $literal_windows } supera el límite de la clave Run o tiene una ruta no válida

journal.cli.host.windows.service.windows.sign.in.resume.failed.windows.error = la reanudación del inicio de sesión de { $literal_windows } falló en { $action } (error de { $literal_windows } { $code })

journal.cli.host.windows.service.windows.sign.in.resume.registry.readback.differed = la relectura del registro de la reanudación del inicio de sesión de { $literal_windows } no coincidió

journal.cli.host.windows.service.your.journal.didn.t.report.ready.so.the.start.can = tu diario no informó que estaba listo, así que no se puede confirmar el inicio. ejecuta `{ $command_solstone_journal_service } logs` para ver por qué

journal.cli.host.windows.service.your.journal.is.ready = tu diario está listo

journal.cli.install.archive.binary = { $binary_name } falta en el archivo comprimido

journal.cli.install.archive.digest = { $algorithm } no coincide: se esperaba { $expected }, se obtuvo { $actual }

journal.cli.install.archive.download = falló la descarga: { $message }

journal.cli.install.archive.host = se rechazó el host de la descarga: { $host }

journal.cli.install.archive.origin = el origen de la descarga no está disponible en { $host }: { $message }

journal.cli.install.archive.path = un miembro del archivo comprimido se sale del destino: { $path }

journal.cli.install.archive.redirect = se superó el límite de saltos de redirección de la descarga: { $limit }

journal.cli.install.archive.scheme = se rechazó el esquema de la descarga para { $host }: { $scheme }

journal.cli.install.archive.size = { $quantity ->
    [one] el tamaño de la descarga no coincide: se esperaba { $expected } byte, se obtuvo { $actual }
    [many] el tamaño de la descarga no coincide: se esperaban { $expected } bytes, se obtuvo { $actual }
   *[other] el tamaño de la descarga no coincide: se esperaban { $expected } bytes, se obtuvo { $actual }
}

journal.cli.install.archive.userinfo = la autoridad de la URL de descarga no debe incluir userinfo: { $authority }

journal.cli.install.backend.metal = el backend debe ser "metal" cuando se proporciona

journal.cli.install.backend.unavailable = el backend local solicitado no está disponible en esta plataforma

journal.cli.install.component.packaged = defecto de compilación: este componente se incluye en el paquete, y reintentar no va a ayudar

journal.cli.install.component.unfetched = defecto de compilación: esta compilación no descarga este componente, y reintentar no va a ayudar

journal.cli.install.coreml.asset.digest = el digest del recurso de parakeet { $coreml } no coincide para { $file }: se esperaba { $expected }, se encontró { $found }

journal.cli.install.coreml.asset.incomplete = el recurso del modelo de habla está incompleto

journal.cli.install.coreml.asset.missing = falta el recurso de parakeet { $coreml }: { $file }; instálalo con: { $install_command }

journal.cli.install.coreml.asset.read = no se pudo leer { $file }: { $error }

journal.cli.install.coreml.asset.size = el tamaño del recurso de parakeet { $coreml } no coincide para { $file }: se esperaba { $expected }, se encontró { $found }

journal.cli.install.coreml.attempt.changed = el intento de instalación de habla cambió.

journal.cli.install.coreml.cache.directory = el directorio de caché del centinela de parakeet { $coreml } ({ $installed }) no coincide con el configurado ({ $configured })

journal.cli.install.coreml.catalog.conflict = las filas del catálogo de { $artifact } tienen versiones en conflicto

journal.cli.install.coreml.catalog.empty = no se encontraron filas del catálogo de { $artifact }

journal.cli.install.coreml.catalog.mismatch = la versión del catálogo de parakeet { $coreml } no coincide: se encontró { $found }, se esperaba { $expected }

journal.cli.install.coreml.platform = parakeet { $coreml } solo es compatible con { $host }, no con { $os_name }/{ $arch }

journal.cli.install.coreml.publication.lock = no se pudo adquirir el bloqueo de publicación: { $error }

journal.cli.install.coreml.publish.restore = { $error }; falló la restauración: { $restore }

journal.cli.install.coreml.sentinel.not.ready = el centinela de parakeet { $coreml } no está listo: { $path }; instálalo con: { $install_command }

journal.cli.install.coreml.sentinel.removed = se quitó el centinela vigente antes de la publicación

journal.cli.install.cuda.integrity = fallo de integridad del entorno de ejecución cuda

journal.cli.install.disk.insufficient = no hay espacio suficiente para terminar de descargar el modelo

journal.cli.install.fingerprint.artifact = { $field } falta

journal.cli.install.fingerprint.backend = falta el backend

journal.cli.install.fingerprint.missing = falta la huella del intento

journal.cli.install.fingerprint.runtime = falta la clave del artefacto del entorno de ejecución

journal.cli.install.fit.artifact.local = artefactos del proveedor local

journal.cli.install.fit.artifact.parakeet = artefactos de parakeet.cpp

journal.cli.install.fit.artifact.rfdetr = artefactos de rf-detr.cpp

journal.cli.install.fit.check.disk = disco

journal.cli.install.fit.check.gpu = gpu

journal.cli.install.fit.check.platform = plataforma

journal.cli.install.fit.check.ram = ram

journal.cli.install.fit.disk.insufficient = espacio en disco insuficiente para { $requirement } (se necesitan { $required } GB, hay { $available } GB libres)

journal.cli.install.fit.disk.insufficient.named = espacio en disco insuficiente para { $requirement } (se necesitan { $required } GB, hay { $available } GB libres): { $known }

journal.cli.install.fit.disk.insufficient.named.unknown = espacio en disco insuficiente para { $requirement } (se necesitan { $required } GB, hay { $available } GB libres): { $known }; tamaño de descarga desconocido para { $unknown }

journal.cli.install.fit.disk.insufficient.unknown = espacio en disco insuficiente para { $requirement } (se necesitan { $required } GB, hay { $available } GB libres); tamaño de descarga desconocido para { $unknown }

journal.cli.install.fit.disk.item.cuda.tarball = Archivo comprimido CUDA de llama-server

journal.cli.install.fit.disk.item.gguf.model = Modelo GGUF

journal.cli.install.fit.disk.item.llama.tarball = archivo comprimido de llama-server

journal.cli.install.fit.disk.item.mlx.model = Modelo MLX

journal.cli.install.fit.disk.item.mmproj = mmproj

journal.cli.install.fit.disk.item.parakeet.cpu = archivo comprimido del servidor parakeet para CPU

journal.cli.install.fit.disk.item.parakeet.model = modelo GGUF de parakeet

journal.cli.install.fit.disk.item.parakeet.vulkan = archivo comprimido del servidor parakeet para Vulkan

journal.cli.install.fit.disk.item.rfdetr.cli = binario de la CLI de rf-detr

journal.cli.install.fit.disk.item.rfdetr.model = modelo GGUF de rf-detr

journal.cli.install.fit.disk.ok = { $available } GB libres para { $required } GB de { $requirement }

journal.cli.install.fit.disk.ok.unknown = { $available } GB libres; para { $requirement } se necesitan { $required } GB; tamaño de descarga desconocido para { $unknown }

journal.cli.install.fit.disk.overflow = desbordamiento del espacio en disco disponible

journal.cli.install.fit.disk.requirement.bundled = instalación de recursos incluidos

journal.cli.install.fit.disk.requirement.downloads = descargas conocidas

journal.cli.install.fit.disk.unavailable = no se pudo verificar el espacio en disco disponible en { $path }: { $error }

journal.cli.install.fit.disk.unavailable.unknown = no se pudo verificar el espacio en disco disponible en { $path }: { $error }; tamaño de descarga desconocido para { $unknown }

journal.cli.install.fit.disk.unsupported = la inspección del espacio en disco no es compatible con esta plataforma

journal.cli.install.fit.gpu.cuda.memory.unknown = el backend resuelto es { $backend }: { $reason }; la memoria de la GPU es desconocida

journal.cli.install.fit.gpu.cuda.ok = Se seleccionó el backend CUDA: { $reason }

journal.cli.install.fit.gpu.cuda.ok.cpu = Se seleccionó el backend CUDA: { $reason }; un modelo se ejecuta en tu GPU; la transcripción se ejecuta en tu CPU en este equipo

journal.cli.install.fit.gpu.cuda.ok.unified = Se seleccionó el backend CUDA: { $reason }; la memoria de escalonado de la GPU usa MemAvailable del sistema

journal.cli.install.fit.gpu.cuda.ok.unified.cpu = Se seleccionó el backend CUDA: { $reason }; la memoria de escalonado de la GPU usa MemAvailable del sistema; un modelo se ejecuta en tu GPU; la transcripción se ejecuta en tu CPU en este equipo

journal.cli.install.fit.gpu.cuda.probe.unavailable = La sonda de la GPU NVIDIA no está disponible; el backend resuelto es { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.below.bar = La GPU { $name } tiene { $available } GB; los modelos locales incluidos necesitan al menos 6 GB; el backend resuelto es { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.below.bar.cpu = La GPU { $name } tiene { $available } GB; los modelos locales incluidos necesitan al menos 6 GB; el backend resuelto es { $backend }: { $reason }; un modelo se ejecuta en tu GPU; la transcripción se ejecuta en tu CPU en este equipo

journal.cli.install.fit.gpu.vulkan.eligible = Se seleccionó la GPU Vulkan: { $name }; el backend resuelto es { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.eligible.cpu = Se seleccionó la GPU Vulkan: { $name }; el backend resuelto es { $backend }: { $reason }; un modelo se ejecuta en tu GPU; la transcripción se ejecuta en tu CPU en este equipo

journal.cli.install.fit.gpu.vulkan.memory.unknown = No se pudo leer la memoria de la GPU; el backend resuelto es { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.no.hardware = no se seleccionó una GPU Vulkan de hardware; el backend resuelto es { $backend }: { $reason }

journal.cli.install.fit.gpu.vulkan.probe.incomplete = La sonda de la GPU Vulkan no terminó; el backend resuelto es { $backend }: { $reason }

journal.cli.install.fit.header = { $artifact } revisión de compatibilidad: { $overall }

journal.cli.install.fit.platform.embedded.blocked = la app del diario no proporciona este entorno de ejecución local

journal.cli.install.fit.platform.embedded.ok = el entorno de ejecución local está incluido en la app del diario

journal.cli.install.fit.platform.llama.blocked = No hay un artefacto anclado de llama-server para la plataforma { $key }

journal.cli.install.fit.platform.llama.ok = el artefacto anclado de llama-server está disponible para { $key }

journal.cli.install.fit.platform.mlx.blocked = MLX requiere macos en Apple silicon; se obtuvo { $os }/{ $arch }

journal.cli.install.fit.platform.mlx.ok = El entorno de ejecución MLX está incluido con el diario

journal.cli.install.fit.platform.parakeet.ok = los artefactos anclados de parakeet.cpp están disponibles para { $key }

journal.cli.install.fit.platform.rfdetr.blocked = rf-detr.cpp requiere darwin/arm64, linux/x86_64 o linux/arm64; se obtuvo { $os }/{ $arch }

journal.cli.install.fit.platform.rfdetr.ok = los artefactos anclados de rf-detr.cpp están disponibles para { $key }

journal.cli.install.fit.platform.rfdetr.windows.ok = la carga útil firmada del paquete de rf-detr.cpp es compatible con { $os }/{ $arch }

journal.cli.install.fit.platform.windows.llama.blocked = llama-server requiere una arquitectura de windows compatible; se obtuvo { $os }/{ $arch }

journal.cli.install.fit.platform.windows.llama.ok = la carga útil firmada del paquete de llama-server es compatible con windows/{ $arch }

journal.cli.install.fit.ram.insufficient = memoria insuficiente para { $model } (se necesitan { $required } GB disponibles, hay { $available } GB disponibles)

journal.cli.install.fit.ram.ok = { $available } GB de memoria disponible cumple el requisito de { $required } GB para { $model }

journal.cli.install.fit.ram.unknown = no se pudo verificar la memoria disponible para { $model }

journal.cli.install.fit.row = [{ $severity }] { $name }: { $detail }

journal.cli.install.fit.severity.blocked = bloqueado

journal.cli.install.fit.severity.ok = ok

journal.cli.install.fit.severity.unknown = desconocido

journal.cli.install.fit.severity.warning = aviso

journal.cli.install.hardware.apple.memory = el razonamiento local necesita al menos { $minimum } de memoria

journal.cli.install.hardware.memory.minimum = { $available_bytes ->
    [one] { $vram } local del dispositivo por debajo del mínimo de { $minimum }: { $available_bytes } byte
    [many] { $vram } local del dispositivo por debajo del mínimo de { $minimum }: { $available_bytes } bytes
   *[other] { $vram } local del dispositivo por debajo del mínimo de { $minimum }: { $available_bytes } bytes
}

journal.cli.install.hardware.memory.unknown = la sonda de { $vulkan } no pudo informar la memoria

journal.cli.install.hardware.no.hardware = no se encontró una GPU de hardware { $vulkan }

journal.cli.install.hardware.probe.required = se requiere una observación de la GPU { $vulkan } en Windows

journal.cli.install.lease.held = la reserva de instalación de { $provider } está retenida

journal.cli.install.lease.regular = la reserva de instalación no es un archivo regular

journal.cli.install.member.cuda.arch = arquitectura cuda no compatible: { $arch }

journal.cli.install.member.cuda.arch.key = clave de arquitectura cuda no compatible: { $artifact_key }

journal.cli.install.member.cuda.missing = falta el miembro cuda requerido: { $name }

journal.cli.install.member.cuda.regular = el miembro cuda requerido no es un archivo regular: { $name }

journal.cli.install.member.link.broken = enlace roto en la cadena de { $name }

journal.cli.install.member.link.cycle = se detectó un ciclo de enlaces para { $name }

journal.cli.install.member.link.missing = falta el miembro de enlace requerido: { $name }

journal.cli.install.member.link.read = no se puede leer el enlace { $name }: { $error }

journal.cli.install.member.link.regular = el enlace { $name } no se resuelve en un archivo regular

journal.cli.install.member.link.target = el destino del enlace { $name } no coincide: se esperaba { $target }, se obtuvo { $found }

journal.cli.install.member.link.type = el miembro de enlace requerido no es un enlace simbólico: { $name }

journal.cli.install.member.link.unreadable = no se puede leer el enlace de la cadena de { $name }

journal.cli.install.member.regular.missing = falta el miembro regular requerido: { $name }

journal.cli.install.member.regular.type = el miembro regular requerido no es un archivo regular: { $name }

journal.cli.install.metal.platform = la inferencia local de { $metal } no es compatible con { $key }

journal.cli.install.mlx.digest.read = { $function } falló para { $file }: { $error }

journal.cli.install.mlx.directory.explicit = el directorio explícito del modelo { $mlx } no existe: { $path }

journal.cli.install.mlx.directory.invalid = el directorio del modelo no es un directorio: { $path }

journal.cli.install.mlx.directory.override = { $env } no existe: { $path }

journal.cli.install.mlx.directory.read = no se puede leer el directorio del modelo: { $error }

journal.cli.install.mlx.directory.unset = { $env } no tiene valor o no es un directorio

journal.cli.install.mlx.entry.read = no se pudo leer la entrada: { $error }

journal.cli.install.mlx.file.digest = el sha256 no coincide para { $file }: se esperaba { $expected }, se obtuvo { $actual }

journal.cli.install.mlx.file.missing = falta el archivo requerido: { $file }

journal.cli.install.mlx.file.regular = no es un archivo regular: { $file }

journal.cli.install.mlx.file.size = el tamaño no coincide para { $file }: se esperaba { $expected }, se obtuvo { $actual }

journal.cli.install.mlx.helper.explicit = el auxiliar explícito de { $mlx } no es válido o falta { $library }: { $path }

journal.cli.install.mlx.helper.invalid = el binario auxiliar no es válido: { $path }

journal.cli.install.mlx.helper.missing = no se encontró el ejecutable { $helper } con el paquete de recursos { $library }

journal.cli.install.mlx.helper.override = { $env } no es válido o falta { $library }: { $path }

journal.cli.install.mlx.input.extra = entrada de modelo adicional sin verificar: { $file }

journal.cli.install.mlx.metadata.read = no se pueden leer los metadatos de { $file }: { $error }

journal.cli.install.mlx.override.failed = falló la verificación del reemplazo de { $mlx }

journal.cli.install.mlx.pins.empty = lista de anclajes vacía

journal.cli.install.mlx.pins.missing = no se proporcionaron anclajes

journal.cli.install.mlx.platform = { $mlx } requiere { $os }

journal.cli.install.mlx.type.read = falló el tipo de archivo: { $error }

journal.cli.install.pdfium.windows.executable.location = el ejecutable del diario en ejecución no está en el directorio bin del paquete: { $path }

journal.cli.install.pdfium.windows.executable.parent = el ejecutable del diario en ejecución no tiene un directorio contenedor: { $path }

journal.cli.install.pdfium.windows.executable.read = no se pudo determinar el ejecutable del diario en ejecución: { $error }

journal.cli.install.pdfium.windows.package.root = el directorio bin del paquete no tiene una raíz de paquete: { $path }

journal.cli.install.pdfium.windows.payload.refuses = la carga útil firmada de la app de PDF rechaza { $member }: { $error }

journal.cli.install.pdfium.windows.payload.verify = no se pudo verificar la carga útil firmada de la app de PDF: { $error }

journal.cli.install.pdfium.windows.runtime = La verificación del paquete PDF de Windows requiere un entorno de ejecución de Windows

journal.cli.install.pin.missing = no hay un anclaje para { $key }

journal.cli.install.registry.artifact.ambiguous = el catálogo tiene varios artefactos { $unit } que coinciden

journal.cli.install.registry.artifact.missing = el catálogo no tiene un artefacto { $unit } que coincida

journal.cli.install.registry.platform = no hay una asignación de plataforma del catálogo para { $key }

journal.cli.install.rfdetr.asset.missing = falta el recurso de rf-detr: { $label }

journal.cli.install.rfdetr.binary.missing = El archivo comprimido extraído no contenía { $binary }

journal.cli.install.rfdetr.bundled.missing = falta el recurso incluido de rf-detr: { $filename }; vuelve a instalar el paquete del diario

journal.cli.install.rfdetr.contract.missing = falta el contrato de entrega compilado de rf-detr

journal.cli.install.rfdetr.contract.slot = la ranura { $actual } del contrato de entrega compilado de rf-detr no coincide con { $expected }

journal.cli.install.rfdetr.member.mismatch = el miembro binario { $actual } de rf-detr no coincide con { $expected }

journal.cli.install.rfdetr.member.outside = el binario de rf-detr estaba fuera de la raíz de extracción: { $error }

journal.cli.install.rfdetr.probe.failed = falló la sonda del motor de rf-detr: { $result }

journal.cli.install.rfdetr.sha256.mismatch = el sha256 no coincide para { $label }

journal.cli.install.rfdetr.sidecar.invalid = sidecar de rf-detr no válido: { $path }: { $error }

journal.cli.install.rfdetr.sidecar.mismatch = el sidecar de rf-detr no coincide con los artefactos anclados

journal.cli.install.rfdetr.sidecar.missing = falta el sidecar de rf-detr: { $path }

journal.cli.install.rfdetr.size.mismatch = el tamaño no coincide para { $label }: se esperaba { $expected }, se obtuvo { $actual }

journal.cli.install.rfdetr.windows.detect.failed = falló rfdetr-cli detect

journal.cli.install.rfdetr.windows.executable.location = el ejecutable del diario en ejecución no está en el directorio bin del paquete: { $path }

journal.cli.install.rfdetr.windows.executable.parent = el ejecutable del diario en ejecución no tiene un directorio contenedor

journal.cli.install.rfdetr.windows.executable.read = no se pudo determinar el ejecutable del diario en ejecución: { $error }

journal.cli.install.rfdetr.windows.package.root = el directorio bin del paquete no tiene una raíz de paquete

journal.cli.install.rfdetr.windows.result.limit = El resultado de RF-DETR superó su límite de bytes

journal.cli.install.rfdetr.windows.runtime = La verificación del paquete RF-DETR de Windows requiere un entorno de ejecución de Windows

journal.cli.install.rfdetr.windows.verify.failed = Falló la verificación del paquete RF-DETR de Windows: { $error }

journal.cli.install.runtime.unavailable = la app del diario no proporciona este entorno de ejecución local

journal.cli.install.speech.platform = { $program } no es compatible con { $os_name }/{ $arch }

journal.cli.install.staging.binary.parent = el binario no tiene directorio padre

journal.cli.install.staging.bundle.occupied = el destino del paquete ya existe: { $path }

journal.cli.install.staging.target.name = el destino no tiene nombre de archivo

journal.cli.install.staging.target.parent = el destino no tiene directorio padre

journal.cli.install.status.conflict.different.attempt.while.install.in.flight = conflicto de estado de la instalación: intento distinto durante una instalación en curso

journal.cli.install.status.conflict.illegal.install.status.transition = conflicto de estado de la instalación: transición de estado de la instalación no permitida

journal.cli.install.status.conflict.install.attempt.id.changed = conflicto de estado de la instalación: cambió el id del intento de instalación

journal.cli.install.status.conflict.install.attempt.is.no.longer.in.flight = conflicto de estado de la instalación: el intento de instalación ya no está en curso

journal.cli.install.status.conflict.install.progress.can.only.be.bumped.for = conflicto de estado de la instalación: el progreso de la instalación solo se puede incrementar en estados en curso

journal.cli.install.status.conflict.install.target.fingerprint.changed = conflicto de estado de la instalación: cambió la huella del destino de la instalación

journal.cli.install.status.conflict.interrupted.attempt.id.does.not.match = conflicto de estado de la instalación: el id del intento interrumpido no coincide

journal.cli.install.status.conflict.interrupted.target.fingerprint.does.not.match = conflicto de estado de la instalación: la huella del destino interrumpido no coincide

journal.cli.install.status.conflict.new.in.flight.attempt.reused.attempt.id = conflicto de estado de la instalación: un intento nuevo en curso reutilizó el id del intento

journal.cli.install.status.conflict.non.idle.install.status.requires.attempt.id = conflicto de estado de la instalación: un estado de instalación no inactivo requiere un id de intento

journal.cli.install.status.conflict.only.in.flight.installs.can.be.interrupted = conflicto de estado de la instalación: solo se pueden interrumpir instalaciones en curso

journal.cli.install.status.conflict.stale.install.status.revision = conflicto de estado de la instalación: revisión desactualizada del estado de la instalación

journal.cli.install.status.interrupted = se interrumpió la descarga

journal.cli.install.status.lock = error del bloqueo de estado: { $error }

journal.cli.install.status.malformed.file = estado de instalación mal formado: { $path }

journal.cli.install.status.malformed.initial.install.attempt.state.must.be.in = estado de instalación mal formado: el estado inicial del intento de instalación debe estar en curso

journal.cli.install.status.malformed.invalid.provider = estado de instalación mal formado: estado no válido para { $provider }

journal.cli.install.status.malformed.owner.must.be.an.object.or.null = estado de instalación mal formado: owner debe ser un objeto o null

journal.cli.install.status.malformed.providers = estado de instalación mal formado: el estado de instalación del proveedor debe ser uno de: [{ $listed }]

journal.cli.install.status.malformed.state = estado de instalación mal formado: estado de instalación desconocido: { $state }

journal.cli.install.terminal.write = falló la instalación nativa y no se pudo escribir el estado

journal.cli.install.windows.digest.absent = el manifiesto de la carga útil firmada no declara el digest de { $path }

journal.cli.install.windows.executable.location = el ejecutable del diario en ejecución no está en el directorio bin del paquete: { $path }

journal.cli.install.windows.executable.parent = el ejecutable del diario en ejecución no tiene un directorio contenedor

journal.cli.install.windows.executable.read = no se pudo determinar el ejecutable del diario en ejecución: { $error }

journal.cli.install.windows.package.root = el directorio bin del paquete no tiene una raíz de paquete

journal.cli.install.windows.package.runtime = La verificación del paquete de razonamiento local de Windows requiere un entorno de ejecución de Windows

journal.cli.install.windows.payload.bytes = bytes{ $line_break }  { $detail }

journal.cli.install.windows.payload.case.collision = colisión de mayúsculas{ $line_break }  { $detail }

journal.cli.install.windows.payload.digest = digest{ $line_break }  { $detail }

journal.cli.install.windows.payload.empty = vacío{ $line_break }  { $detail }

journal.cli.install.windows.payload.lock.digest = cargo-lock-sha256{ $line_break }  { $detail }

journal.cli.install.windows.payload.manifest = manifiesto{ $line_break }  { $detail }

journal.cli.install.windows.payload.missing = falta{ $line_break }  { $detail }

journal.cli.install.windows.payload.missing.member = miembro faltante{ $line_break }  { $detail }

journal.cli.install.windows.payload.not.regular = no regular{ $line_break }  { $detail }

journal.cli.install.windows.payload.reparse.or.symlink = reparse o enlace simbólico{ $line_break }  { $detail }

journal.cli.install.windows.payload.schema = esquema{ $line_break }  { $detail }

journal.cli.install.windows.payload.signature = firma{ $line_break }  { $detail }

journal.cli.install.windows.payload.source.commit = commit de origen{ $line_break }  { $detail }

journal.cli.install.windows.payload.target = destino{ $line_break }  { $detail }

journal.cli.install.windows.payload.unexpected.member = miembro inesperado{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsafe.path = ruta no segura{ $line_break }  { $detail }

journal.cli.install.windows.payload.unsorted.or.duplicate = desordenado o duplicado{ $line_break }  { $detail }

journal.cli.install.windows.platform = plataforma de Windows no compatible: { $key }

journal.cli.install.windows.target.different = el destino de la carga útil firmada difiere de { $key }

journal.cli.install.windows.target.unsupported = el destino de la carga útil firmada no es compatible

journal.cli.path.create = no se pudo crear el directorio del diario ({ $source }): { $path }: { $error }

journal.cli.path.home = journal-path falló: no se pudo determinar el directorio de inicio

journal.cli.path.utf8 = journal-path falló: la configuración no es un { $encoding } válido
