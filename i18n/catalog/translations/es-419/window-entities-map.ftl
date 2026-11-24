journal.window.entities.accessible.label.dismiss = descartar

journal.window.entities.accessible.name.close = cerrar

journal.window.entities.accessible.name.entity.folders = carpetas de entidades

journal.window.entities.accessible.name.search.detected.entities = buscar entidades detectadas

journal.window.entities.accessible.name.search.entities = buscar entidades

journal.window.entities.accessible.name.search.entity.cards = buscar tarjetas de entidades

journal.window.entities.accessible.name.sort.entities = ordenar entidades

journal.window.entities.aliases.other.names = también se conoce como: { $aliases }

journal.window.entities.button.detail.reattach = volver a agregar

journal.window.entities.button.label.attach.to.facet = adjuntar a la faceta

journal.window.entities.button.label.cancel = cancelar

journal.window.entities.button.label.detach = separar

journal.window.entities.button.label.hide = ocultar

journal.window.entities.button.label.next = siguiente

journal.window.entities.button.label.previous = anterior

journal.window.entities.button.label.save.changes = guardar cambios

journal.window.entities.column.heading.description = descripción

journal.window.entities.column.heading.name = nombre

journal.window.entities.connections.calendar.last.day = { $day }

journal.window.entities.connections.evidence.show.more =
    { $shown_quantity ->
        [one] mostrando { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. mostrar más
        [many] mostrando { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. mostrar más
       *[other] mostrando { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }. mostrar más
    }

journal.window.entities.connections.row.attendance =
    { $total_quantity ->
        [one] 1 evento en común · { $day }
        [many] { $total } eventos en común · { $day }
       *[other] { $total } eventos en común · { $day }
    }

journal.window.entities.connections.row.direct =
    { $total_quantity ->
        [one] 1 momento · { $day }
        [many] { $total } momentos · { $day }
       *[other] { $total } momentos · { $day }
    }

journal.window.entities.connections.summary =
    { $direct_quantity ->
        [one] { $direct } con evidencia directa · { $shared_quantity ->
            [one] { $shared }
            [many] { $shared }
           *[other] { $shared }
        } solo de eventos compartidos
        [many] { $direct } con evidencia directa · { $shared_quantity ->
            [one] { $shared }
            [many] { $shared }
           *[other] { $shared }
        } solo de eventos compartidos
       *[other] { $direct } con evidencia directa · { $shared_quantity ->
            [one] { $shared }
            [many] { $shared }
           *[other] { $shared }
        } solo de eventos compartidos
    }

journal.window.entities.connections.summary.with.calendar =
    { $direct_quantity ->
        [one] { $direct } con evidencia directa · { $shared_quantity ->
            [one] { $shared }
            [many] { $shared }
           *[other] { $shared }
        } solo de eventos compartidos · { $calendar_quantity ->
            [one] { $calendar }
            [many] { $calendar }
           *[other] { $calendar }
        } solo de tu calendario
        [many] { $direct } con evidencia directa · { $shared_quantity ->
            [one] { $shared }
            [many] { $shared }
           *[other] { $shared }
        } solo de eventos compartidos · { $calendar_quantity ->
            [one] { $calendar }
            [many] { $calendar }
           *[other] { $calendar }
        } solo de tu calendario
       *[other] { $direct } con evidencia directa · { $shared_quantity ->
            [one] { $shared }
            [many] { $shared }
           *[other] { $shared }
        } solo de eventos compartidos · { $calendar_quantity ->
            [one] { $calendar }
            [many] { $calendar }
           *[other] { $calendar }
        } solo de tu calendario
    }

journal.window.entities.connections.you.calendar.latest = solo en tu calendario · lo más reciente: { $day }

journal.window.entities.connections.you.latest.attended.with =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: eventos ({ $day })
        [many] { $moments } momentos en común · lo más reciente: eventos ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: eventos ({ $day })
    }

journal.window.entities.connections.you.latest.co.present =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: cerca juntos ({ $day })
        [many] { $moments } momentos en común · lo más reciente: cerca juntos ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: cerca juntos ({ $day })
    }

journal.window.entities.connections.you.latest.created =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: creado ({ $day })
        [many] { $moments } momentos en común · lo más reciente: creado ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: creado ({ $day })
    }

journal.window.entities.connections.you.latest.fallback =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: { $evidence_name } ({ $day })
        [many] { $moments } momentos en común · lo más reciente: { $evidence_name } ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: { $evidence_name } ({ $day })
    }

journal.window.entities.connections.you.latest.family.of =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: familia ({ $day })
        [many] { $moments } momentos en común · lo más reciente: familia ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: familia ({ $day })
    }

journal.window.entities.connections.you.latest.knows =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: conoce ({ $day })
        [many] { $moments } momentos en común · lo más reciente: conoce ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: conoce ({ $day })
    }

journal.window.entities.connections.you.latest.mentioned =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: mencionado ({ $day })
        [many] { $moments } momentos en común · lo más reciente: mencionado ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: mencionado ({ $day })
    }

journal.window.entities.connections.you.latest.messaged.with =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: mensaje ({ $day })
        [many] { $moments } momentos en común · lo más reciente: mensaje ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: mensaje ({ $day })
    }

journal.window.entities.connections.you.latest.other =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: relacionado ({ $day })
        [many] { $moments } momentos en común · lo más reciente: relacionado ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: relacionado ({ $day })
    }

journal.window.entities.connections.you.latest.party.of =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: parte en ({ $day })
        [many] { $moments } momentos en común · lo más reciente: parte en ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: parte en ({ $day })
    }

journal.window.entities.connections.you.latest.reports.to =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: reporta a ({ $day })
        [many] { $moments } momentos en común · lo más reciente: reporta a ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: reporta a ({ $day })
    }

journal.window.entities.connections.you.latest.scheduled.with =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: agendado ({ $day })
        [many] { $moments } momentos en común · lo más reciente: agendado ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: agendado ({ $day })
    }

journal.window.entities.connections.you.latest.spoke.with =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: habló ({ $day })
        [many] { $moments } momentos en común · lo más reciente: habló ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: habló ({ $day })
    }

journal.window.entities.connections.you.latest.uses =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: usa ({ $day })
        [many] { $moments } momentos en común · lo más reciente: usa ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: usa ({ $day })
    }

journal.window.entities.connections.you.latest.works.at =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: trabaja en ({ $day })
        [many] { $moments } momentos en común · lo más reciente: trabaja en ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: trabaja en ({ $day })
    }

journal.window.entities.connections.you.latest.works.with =
    { $moments_quantity ->
        [one] 1 momento en común · lo más reciente: trabaja con ({ $day })
        [many] { $moments } momentos en común · lo más reciente: trabaja con ({ $day })
       *[other] { $moments } momentos en común · lo más reciente: trabaja con ({ $day })
    }

journal.window.entities.copy.cards.empty = todavía no hay entidades en esta faceta. marca con estrella lo de abajo que quieras conservar, o agrega una tú.

journal.window.entities.copy.conn.ambiguous = esto puede coincidir con más de una entidad.

journal.window.entities.copy.conn.calendar.only = solo calendario

journal.window.entities.copy.conn.empty = todavía no hay conexiones registradas. aparecen cuando esta entidad aparece junto a alguien o a algo más.

journal.window.entities.copy.conn.events.only = solo eventos

journal.window.entities.copy.conn.evidence.caption = los momentos detrás de esto: evidencia, no una conclusión

journal.window.entities.copy.conn.evidence.failed = no se pudieron cargar estos momentos. vuelve a intentarlo.

journal.window.entities.copy.conn.evidence.more = mostrando { $slot1 } de { $slot2 }. mostrar más

journal.window.entities.copy.conn.evidence.view.entity = ver entidad →

journal.window.entities.copy.conn.horizon = las conexiones de tiempo en el mismo lugar solo se remontan a { $slot1 }. los { $slot2 } días anteriores siguen en tu diario; esas conexiones todavía no se han construido.

journal.window.entities.copy.conn.horizon.one = las conexiones de tiempo en el mismo lugar solo se remontan a { $slot1 }. un día anterior sigue en tu diario; esas conexiones todavía no se han construido.

journal.window.entities.copy.conn.index.action = revisar el estado del sistema →

journal.window.entities.copy.conn.index.unavailable = las conexiones no están disponibles ahora mismo.

journal.window.entities.copy.conn.kind.chip.words.mentioned = menciones

journal.window.entities.copy.conn.kind.words.attended.with = eventos

journal.window.entities.copy.conn.kind.words.co.present = cerca juntos

journal.window.entities.copy.conn.kind.words.created = creado

journal.window.entities.copy.conn.kind.words.family.of = familia

journal.window.entities.copy.conn.kind.words.knows = conoce

journal.window.entities.copy.conn.kind.words.mentioned = mencionado

journal.window.entities.copy.conn.kind.words.messaged.with = mensaje

journal.window.entities.copy.conn.kind.words.other = relacionado

journal.window.entities.copy.conn.kind.words.party.of = parte en

journal.window.entities.copy.conn.kind.words.reports.to = reporta a

journal.window.entities.copy.conn.kind.words.scheduled.with = agendado

journal.window.entities.copy.conn.kind.words.spoke.with = habló

journal.window.entities.copy.conn.kind.words.uses = usa

journal.window.entities.copy.conn.kind.words.works.at = trabaja en

journal.window.entities.copy.conn.kind.words.works.with = trabaja con

journal.window.entities.copy.conn.label.fallbacks.co.present = tiempo en el mismo lugar

journal.window.entities.copy.conn.label.fallbacks.mentioned = surgió en la conversación

journal.window.entities.copy.conn.label.fallbacks.spoke.with = una conversación

journal.window.entities.copy.conn.load.failed = no se pudieron cargar las conexiones. vuelve a intentarlo.

journal.window.entities.copy.conn.row.meta = { $slot1 } momentos · { $slot2 }

journal.window.entities.copy.conn.row.meta.attendance = { $slot1 } eventos en común · { $slot2 }

journal.window.entities.copy.conn.row.meta.one = 1 momento · { $slot1 }

journal.window.entities.copy.conn.row.meta.one.attendance = 1 evento en común · { $slot1 }

journal.window.entities.copy.conn.row.meta.scheduled = { $slot1 }

journal.window.entities.copy.conn.source.word.fallback = lo que tu diario guarda

journal.window.entities.copy.conn.source.words.calendar = eventos del calendario

journal.window.entities.copy.conn.source.words.co.presence = momentos compartidos

journal.window.entities.copy.conn.source.words.document = documentos

journal.window.entities.copy.conn.source.words.event.legacy = eventos más antiguos del calendario

journal.window.entities.copy.conn.source.words.mention = conversación

journal.window.entities.copy.conn.source.words.messaging = mensajes

journal.window.entities.copy.conn.source.words.observation = lo que tu diario guarda

journal.window.entities.copy.conn.source.words.participation = registros de actividad

journal.window.entities.copy.conn.source.words.relation = lo que tu diario guarda

journal.window.entities.copy.conn.source.words.speaker = conversación

journal.window.entities.copy.conn.summary = { $slot1 } con evidencia directa · { $slot2 } solo de eventos compartidos

journal.window.entities.copy.conn.summary.calendar = { $slot1 } solo de tu calendario

journal.window.entities.copy.conn.with.you.eyebrow = contigo

journal.window.entities.copy.conn.with.you.failed = no se pudo revisar tu propio historial con esta entidad justo ahora.

journal.window.entities.copy.conn.with.you.summary = { $slot1 } momentos en común · lo más reciente: { $slot2 } ({ $slot3 })

journal.window.entities.copy.conn.with.you.summary.one = 1 momento en común · lo más reciente: { $slot1 } ({ $slot2 })

journal.window.entities.copy.conn.with.you.summary.scheduled = solo en tu calendario · lo más reciente: { $slot1 }

journal.window.entities.copy.conn.with.you.toggle = los momentos detrás de esto ›

journal.window.entities.copy.detach.confirm = ¿separar { $slot1 } de { $slot2 }? sigue en tu diario y puedes volver a agregarla cuando quieras.

journal.window.entities.copy.detach.done = se separó de { $slot1 }. { $slot2 } sigue en tu diario.

journal.window.entities.copy.detach.find.action = buscarla en tu diario →

journal.window.entities.copy.detach.reattach.action = volver a agregarla →

journal.window.entities.copy.detected.all.hidden = todo lo detectado aquí últimamente está oculto.

journal.window.entities.copy.detected.empty = todavía no hay nada aquí. a medida que agregas a tu diario, aparecerán aquí personas, lugares y cosas.

journal.window.entities.copy.detected.hidden.badge = oculto

journal.window.entities.copy.detected.hidden.count = { $slot1 } nombres ocultos en { $slot2 }

journal.window.entities.copy.detected.hidden.count.one = 1 nombre oculto en { $slot1 }

journal.window.entities.copy.detected.hidden.heading = oculto en { $slot1 }

journal.window.entities.copy.detected.hidden.last = se detectó por última vez { $slot1 }

journal.window.entities.copy.detected.hidden.lede = un nombre oculto sigue en tu diario y sigue oculto por más que se lo vuelva a detectar. sus detecciones aquí quedan fuera de la búsqueda, los agentes conectados no pueden llegar a ellas y tu diario no lo agregará a esta faceta.

journal.window.entities.copy.detected.hidden.show = mostrar lo oculto

journal.window.entities.copy.detected.hidden.stop.show = dejar de mostrar lo oculto

journal.window.entities.copy.detected.hide.action = ocultar { $slot1 } en { $slot2 }

journal.window.entities.copy.detected.hide.done = { $slot1 } está oculto en { $slot2 }.

journal.window.entities.copy.detected.hide.done.detail = sigue oculto por más que se lo vuelva a detectar.

journal.window.entities.copy.detected.hide.failed = no se pudo ocultar { $slot1 }. no cambió nada.

journal.window.entities.copy.detected.hide.no.search = la búsqueda y los agentes conectados todavía pueden encontrar sus detecciones aquí.

journal.window.entities.copy.detected.hide.try.again = reintentar

journal.window.entities.copy.detected.still.hidden = { $slot1 } todavía coincide con { $slot2 }, una entidad oculta.

journal.window.entities.copy.detected.still.hidden.open = abrir { $slot1 }

journal.window.entities.copy.detected.unhide = dejar de ocultar

journal.window.entities.copy.detected.unhide.failed = no se pudo dejar de ocultar { $slot1 }. no cambió nada.

journal.window.entities.copy.detected.unreadable = no se pueden mostrar las detecciones aquí. tu diario no pudo leer qué nombres ocultaste en esta faceta.

journal.window.entities.copy.faces.busy = tu diario está ocupado con datos de rostros ahora mismo, así que no cambió nada.

journal.window.entities.copy.faces.change = cambiar foto de perfil

journal.window.entities.copy.faces.changed = esto cambió desde que se cargó la página, así que no se deshizo.

journal.window.entities.copy.faces.chosen.alt = foto de perfil de { $slot1 }

journal.window.entities.copy.faces.confirmed.lede = elegiste esta foto de perfil para { $slot1 }.

journal.window.entities.copy.faces.disabled = las sugerencias de foto de perfil están desactivadas, así que no cambió nada.

journal.window.entities.copy.faces.dismissed.lede = elegiste no usar foto de perfil para { $slot1 }. si aparecen otros rostros que podrían ser { $slot2 }, pueden mostrarse aquí.

journal.window.entities.copy.faces.done.chosen = se guardó la foto de perfil para { $slot1 }.

journal.window.entities.copy.faces.done.dismissed = sin foto de perfil para { $slot1 }.

journal.window.entities.copy.faces.done.rejected = ok, ese no es { $slot1 }.

journal.window.entities.copy.faces.face.alt = un rostro de una reunión el { $slot1 } que podría ser { $slot2 }

journal.window.entities.copy.faces.facet.hint = elige una foto de perfil para { $slot1 }

journal.window.entities.copy.faces.failed = eso no se guardó.

journal.window.entities.copy.faces.from.day = de una reunión el { $slot1 }

journal.window.entities.copy.faces.gone = ese rostro ya no está disponible.

journal.window.entities.copy.faces.heading = foto de perfil

journal.window.entities.copy.faces.load.failed = no se pudieron cargar estos rostros.

journal.window.entities.copy.faces.merge.reason = los rostros de tus reuniones sugieren que pueden ser la misma persona.

journal.window.entities.copy.faces.none = ninguno de estos

journal.window.entities.copy.faces.not.them = no es { $slot1 }

journal.window.entities.copy.faces.possible.lede = estos rostros de tus reuniones podrían ser { $slot1 }. no se usa ninguno hasta que elijas uno.

journal.window.entities.copy.faces.possible.title = elige una foto de perfil para { $slot1 }

journal.window.entities.copy.faces.refresh = actualizar

journal.window.entities.copy.faces.remove = no usar foto de perfil

journal.window.entities.copy.faces.retry = reintentar

journal.window.entities.copy.faces.see.others = elegir otro

journal.window.entities.copy.faces.settings.link = actívalas en configuración

journal.window.entities.copy.faces.show.again = deshacer

journal.window.entities.copy.faces.source.anchored = junto a su nombre

journal.window.entities.copy.faces.source.box = en su recuadro de video

journal.window.entities.copy.faces.stale = esto cambió desde que se cargó la página, así que no se guardó nada.

journal.window.entities.copy.faces.suggested.badge = sugerida

journal.window.entities.copy.faces.suggested.lede = tu diario eligió este rostro de las grabaciones de tus reuniones. es solo una sugerencia hasta que digas que sí.

journal.window.entities.copy.faces.suggested.title = ¿este es { $slot1 }?

journal.window.entities.copy.faces.unconfirmed.alt = un rostro de una reunión que podría ser { $slot1 }, sin confirmar

journal.window.entities.copy.faces.unconfirmed.badge = sin confirmar

journal.window.entities.copy.faces.undo = deshacer

journal.window.entities.copy.faces.undone = deshecho.

journal.window.entities.copy.faces.use = usar esta foto de perfil

journal.window.entities.copy.faces.yes = sí, usar esta foto de perfil

journal.window.entities.copy.grid.clear.range = limpiar rango

journal.window.entities.copy.grid.day.one = día

journal.window.entities.copy.grid.day.other = días

journal.window.entities.copy.grid.default.day = día más reciente: { $slot1 }

journal.window.entities.copy.grid.lede = { $slot1 } { $slot2 } en { $slot3 } { $slot4 }

journal.window.entities.copy.grid.range = { $slot1 } – { $slot2 }

journal.window.entities.copy.grid.show.all = mostrar todos los { $slot1 } { $slot2 }

journal.window.entities.copy.grid.title = momentos a lo largo del tiempo

journal.window.entities.copy.grid.unit.none = todavía no hay nada

journal.window.entities.copy.grid.unit.one = momento

journal.window.entities.copy.grid.unit.other = momentos

journal.window.entities.copy.hide.pane.help = la mueve a lo oculto, donde puedes dejar de ocultarla. sus notas y sus detecciones en sus facetas salen de la búsqueda, y los agentes conectados no pueden llegar a ellas.

journal.window.entities.copy.obs.disclosure = mostrando los { $slot1 } más recientes de { $slot2 } momentos

journal.window.entities.copy.obs.empty = todavía nada con { $slot1 }.

journal.window.entities.copy.obs.heading = momentos

journal.window.entities.copy.obs.load.failed = no se pudieron cargar estos momentos. vuelve a intentarlo →

journal.window.entities.copy.obs.source.link.title = ver este día

journal.window.entities.copy.scope.empty.action = mostrar todo tu diario

journal.window.entities.copy.scope.empty.body = las entidades aparecen aquí a medida que tu diario guarda personas, lugares y proyectos en esta faceta.

journal.window.entities.copy.scope.empty.title = todavía nada en { $slot1 }.

journal.window.entities.copy.scope.facet.missing = esa faceta ya no está en tu diario, así que esto es todo tu diario.

journal.window.entities.copy.scope.showing = mostrando

journal.window.entities.copy.scope.whole.journal = tu diario completo

journal.window.entities.copy.trust.action.failed = ese cambio de entidad no terminó.

journal.window.entities.copy.trust.heading = confianza de identidad

journal.window.entities.copy.trust.history.change = { $slot1 } → { $slot2 }

journal.window.entities.copy.trust.history.empty = todavía no hay historial de identidad disponible.

journal.window.entities.copy.trust.history.heading = historial de versiones

journal.window.entities.copy.trust.history.suggested.by = sugerido por { $slot1 }

journal.window.entities.copy.trust.history.summary = { $slot1 } · { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.identity.none = sin identidad previa

journal.window.entities.copy.trust.identity.summary = { $slot1 } · { $slot2 } · alias: { $slot3 }

journal.window.entities.copy.trust.load.failed = no se pudo cargar los detalles de confianza de la identidad. vuelve a intentarlo.

journal.window.entities.copy.trust.merge.action = previsualizar fusión

journal.window.entities.copy.trust.merge.confirm = ¿fusionar { $slot1 } en { $slot2 }? una fusión no se puede deshacer.

journal.window.entities.copy.trust.merge.confirm.action = confirmar fusión

journal.window.entities.copy.trust.merge.done = se fusionó.

journal.window.entities.copy.trust.merge.label = fusionar en

journal.window.entities.copy.trust.preview.counts = muestras de voz agregadas: { $slot1 } · muestras tras la fusión: { $slot2 } · notas bajo el nombre que se fusiona: { $slot3 } · etiquetas de hablante cambiadas: { $slot4 } · registros de actividad cambiados: { $slot5 }

journal.window.entities.copy.trust.preview.summary = alias agregados: { $slot1 } · correos agregados: { $slot2 }

journal.window.entities.copy.trust.restore.action = restaurar esta versión

journal.window.entities.copy.trust.restore.confirm = ¿restaurar la identidad de esta versión? la identidad actual permanecerá en el historial.

journal.window.entities.copy.trust.suggestion.summary = { $slot1 } → { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.suggestions.empty = ninguna sugerencia de fusión abierta involucra a esta entidad.

journal.window.entities.copy.trust.suggestions.heading = sugerencias de fusión abiertas

journal.window.entities.copy.voice.changed = esa muestra cambió. recarga la lista antes de quitarla.

journal.window.entities.copy.voice.empty = no hay muestras de voz guardadas para esta persona.

journal.window.entities.copy.voice.error = no se pudo leer ni cambiar esas muestras de voz. vuelve a intentarlo.

journal.window.entities.copy.voice.heading = muestras de voz

journal.window.entities.copy.voice.loading = cargando muestras de voz…

journal.window.entities.copy.voice.more = mostrar más

journal.window.entities.copy.voice.open = abrir grabación

journal.window.entities.copy.voice.remove = quitar

journal.window.entities.copy.voice.retry = reintentar

journal.window.entities.copy.voice.sentence = frase { $slot1 }

journal.window.entities.delete.all.items.gone =
    { $removed_quantity ->
        [one] ya no figura aquí.
        [many] ya no figuran aquí.
       *[other] ya no figuran aquí.
    }

journal.window.entities.delete.couldn.t.erase.that = no se pudo borrar esa entidad

journal.window.entities.delete.couldn.t.recover.that = no se pudo recuperar esa entidad

journal.window.entities.delete.detail.action.help = { $action_start }eliminar{ $action_end } { $description_start }la mueve a la papelera, donde puedes recuperarla hasta que se borre.{ $description_end }

journal.window.entities.delete.erased.from.your.journal =
    { $total_quantity ->
        [one] se borró { $total } entidad de tu diario
        [many] se borraron { $total } entidades de tu diario
       *[other] se borraron { $total } entidades de tu diario
    }

journal.window.entities.delete.erased.of =
    { $processed_quantity ->
        [one] se borró { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
        [many] se borraron { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
       *[other] se borraron { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.entities.delete.erases.unless.you.recover.it = se borra el { $erase_day } a menos que la recuperes.

journal.window.entities.delete.facet.many.its.place.and.notes.in =
    { $facets_quantity ->
        [one] su lugar y sus notas en { $facets } faceta
        [many] su lugar y sus notas en { $facets } facetas
       *[other] su lugar y sus notas en { $facets } facetas
    }

journal.window.entities.delete.gone.more.many.more.are.no.longer.listed =
    { $gone_quantity ->
        [one] { $gone } más ya no figura.
        [many] { $gone } más ya no figuran.
       *[other] { $gone } más ya no figuran.
    }

journal.window.entities.delete.items.find.after.failed.erase =
    { $remaining_quantity ->
        [one] búscala abajo o en la papelera.
        [many] búscalas abajo o en la papelera.
       *[other] búscalas abajo o en la papelera.
    }

journal.window.entities.delete.many.delete.entities =
    { $total_quantity ->
        [one] ¿eliminar { $total } entidad?
        [many] ¿eliminar { $total } entidades?
       *[other] ¿eliminar { $total } entidades?
    }

journal.window.entities.delete.many.erase.entities.now =
    { $total_quantity ->
        [one] ¿borrar { $total } entidad ahora?
        [many] ¿borrar { $total } entidades ahora?
       *[other] ¿borrar { $total } entidades ahora?
    }

journal.window.entities.delete.marked.many.couldn.t.be.moved.and =
    { $remaining_quantity ->
        [one] { $remaining } no se pudo mover y está marcada abajo.
        [many] { $remaining } no se pudieron mover y están marcadas abajo.
       *[other] { $remaining } no se pudieron mover y están marcadas abajo.
    }

journal.window.entities.delete.more.and.more =
    { $more_quantity ->
        [one] y { $more } más
        [many] y { $more } más
       *[other] y { $more } más
    }

journal.window.entities.delete.moved.of.to.the =
    { $processed_quantity ->
        [one] se movió { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } a la papelera
        [many] se movieron { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } a la papelera
       *[other] se movieron { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } a la papelera
    }

journal.window.entities.delete.moved.to.the.trash =
    { $total_quantity ->
        [one] se movió { $total } entidad a la papelera
        [many] se movieron { $total } entidades a la papelera
       *[other] se movieron { $total } entidades a la papelera
    }

journal.window.entities.delete.none.still.many.are.still.in.your.journal =
    { $remaining_quantity ->
        [one] { $remaining } sigue en tu diario, marcada abajo.
        [many] { $remaining } siguen en tu diario, marcadas abajo.
       *[other] { $remaining } siguen en tu diario, marcadas abajo.
    }

journal.window.entities.delete.noun.erased.from.your.journal = entidad borrada de tu diario

journal.window.entities.delete.other.items.gone =
    { $other_quantity ->
        [one] la otra ya no figura aquí.
        [many] las otras ya no figuran aquí.
       *[other] las otras ya no figuran aquí.
    }

journal.window.entities.delete.partial.items.not.erased =
    { $remaining_quantity ->
        [one] la otra no se borró. búscala abajo o en la papelera.
        [many] las otras no se borraron. búscalas abajo o en la papelera.
       *[other] las otras no se borraron. búscalas abajo o en la papelera.
    }

journal.window.entities.delete.partial.items.not.recovered =
    { $remaining_quantity ->
        [one] la otra sigue en la papelera. puedes volver a intentarlo desde ahí.
        [many] las otras siguen en la papelera. puedes volver a intentarlo desde ahí.
       *[other] las otras siguen en la papelera. puedes volver a intentarlo desde ahí.
    }

journal.window.entities.delete.pick.label.select.name = seleccionar { $entity_name }

journal.window.entities.delete.recovered.count =
    { $total_quantity ->
        [one] { $total } entidad recuperada
        [many] { $total } entidades recuperadas
       *[other] { $total } entidades recuperadas
    }

journal.window.entities.delete.recovered.of =
    { $processed_quantity ->
        [one] se recuperó { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
        [many] se recuperaron { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
       *[other] se recuperaron { $processed } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.entities.delete.recovered.one = entidad recuperada

journal.window.entities.delete.selected.selected =
    { $selected_quantity ->
        [one] { $selected } seleccionada
        [many] { $selected } seleccionadas
       *[other] { $selected } seleccionadas
    }

journal.window.entities.delete.still.many.couldn.t.be.moved.and =
    { $remaining_quantity ->
        [one] { $remaining } no se pudo mover y sigue en tu diario, marcada abajo.
        [many] { $remaining } no se pudieron mover y siguen en tu diario, marcadas abajo.
       *[other] { $remaining } no se pudieron mover y siguen en tu diario, marcadas abajo.
    }

journal.window.entities.delete.they.erase.unless.you.recover = se borran el { $erase_day } a menos que las recuperes.

journal.window.entities.detail.aliases.other.names = También se conoce como: { $aliases }

journal.window.entities.detail.facets.count =
    { $facets_quantity ->
        [one] 1 faceta
        [many] { $facets } facetas
       *[other] { $facets } facetas
    }

journal.window.entities.detail.id.description = este nombre parece un ID.

journal.window.entities.detail.last.mentioned.date = Última mención: { $day }

journal.window.entities.detail.last.mentioned.full.day = Última mención: { $day }

journal.window.entities.detected.hidden.heading = ocultos en { $facet_name }

journal.window.entities.detected.hidden.in.facet =
    { $hidden_quantity ->
        [one] 1 nombre oculto en { $facet_name }
        [many] { $hidden } nombres ocultos en { $facet_name }
       *[other] { $hidden } nombres ocultos en { $facet_name }
    }

journal.window.entities.detected.hidden.last.detected = última detección { $day }

journal.window.entities.detected.hidden.reappeared = { $entity_name } todavía coincide con { $matched_name }, una entidad oculta.

journal.window.entities.detected.hidden.reappeared.open = abrir { $matched_name }

journal.window.entities.detected.hide.action = ocultar { $entity_name } en { $facet_name }

journal.window.entities.detected.hide.done = se ocultó { $entity_name } en { $facet_name }.

journal.window.entities.detected.hide.failed = no se pudo ocultar { $entity_name }. no cambió nada.

journal.window.entities.detected.page.range =
    { $total_quantity ->
        [one] mostrando { $first }–{ $last } de { $total } entidad detectada
        [many] mostrando { $first }–{ $last } de { $total } entidades detectadas
       *[other] mostrando { $first }–{ $last } de { $total } entidades detectadas
    }

journal.window.entities.detected.unhide.failed = no se pudo dejar de ocultar { $entity_name }. no cambió nada.

journal.window.entities.entity.delete.copy.again.if.your.journal.finds.it.again = si tu diario la vuelve a encontrar más adelante, se agrega como una entidad nueva.

journal.window.entities.entity.delete.copy.again.many.any.of.them.your.journal.finds = cualquiera de ellas que tu diario vuelva a encontrar más adelante se agrega como una entidad nueva.

journal.window.entities.entity.delete.copy.back.back = atrás

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.it.right = algo más la está cambiando en este momento. vuelve a intentarlo en un minuto.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right = otra cosa está cambiando esto ahora mismo, así que esto no se movió. vuelve a intentarlo en un minuto.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right.variant2 = otra cosa está cambiando esto ahora mismo, así que esto no se borró. vuelve a intentarlo en un minuto.

journal.window.entities.entity.delete.copy.button.delete = eliminar

journal.window.entities.entity.delete.copy.cancel.cancel = cancelar

journal.window.entities.entity.delete.copy.delete.delete = eliminar

journal.window.entities.entity.delete.copy.details.its.details.and.version.history = sus detalles y su historial de versiones

journal.window.entities.entity.delete.copy.done.done = listo

journal.window.entities.entity.delete.copy.erase.now.erase.now = borrar ahora

journal.window.entities.entity.delete.copy.facet.one.its.place.and.notes.in.facet = su lugar y sus notas en 1 faceta

journal.window.entities.entity.delete.copy.facet.unknown.its.place.and.notes.in.each = su lugar y sus notas en cada faceta en la que está

journal.window.entities.entity.delete.copy.gone.it.s.no.longer.in.your = ya no está en tu diario.

journal.window.entities.entity.delete.copy.last.facet.your.journal.needs.at.least.one = tu diario necesita al menos una faceta que no esté silenciada.

journal.window.entities.entity.delete.copy.lead.this.erases.the.entity.s.own = esto borra lo propio de la entidad:

journal.window.entities.entity.delete.copy.lead.this.moves.the.entity.s.own = esto mueve lo propio de la entidad:

journal.window.entities.entity.delete.copy.move.to.trash.move.to.trash = mover a la papelera

journal.window.entities.entity.delete.copy.open.trash.open.trash = abrir la papelera

journal.window.entities.entity.delete.copy.other.this.wasn.t.erased.try.again = esto no se borró. vuelve a intentarlo.

journal.window.entities.entity.delete.copy.other.this.wasn.t.moved.try.again = esto no se movió. vuelve a intentarlo.

journal.window.entities.entity.delete.copy.own.many.this.erases.each.one.s.own = esto borra, de cada una, sus detalles, su historial de versiones, su lugar y sus notas en cada faceta, y lo que tu diario guarda sobre su voz, si hay algo.

journal.window.entities.entity.delete.copy.own.many.this.moves.each.one.s.own = esto mueve, de cada una, sus detalles, su historial de versiones, su lugar y sus notas en cada faceta, y lo que tu diario guarda sobre su voz, si hay algo.

journal.window.entities.entity.delete.copy.principal.this.is.you.so.it.can = esto eres tú, así que no se puede eliminar.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and = sale de tu diario ahora y no se puede recuperar.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and.variant2 = sale de tu diario ahora, espera en la papelera y después se borra. puedes recuperarla hasta entonces.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and = salen de tu diario ahora y no se pueden recuperar.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and.variant2 = salen de tu diario ahora, esperan en la papelera y después se borran. puedes recuperar cada una hasta entonces.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its = no se pudo actualizar el registro de entidades de tu diario. no se movió nada.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant2 = no se pudo actualizar el registro de entidades de tu diario. no se borró nada.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant3 = no se pudo actualizar el registro de entidades de tu diario, así que esto no se movió.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant4 = no se pudo actualizar el registro de entidades de tu diario, así que esto no se borró.

journal.window.entities.entity.delete.copy.recover.recover = recuperar

journal.window.entities.entity.delete.copy.select.all.page.select.all.on.this.page = seleccionar todo en esta página

journal.window.entities.entity.delete.copy.select.all.select.all = seleccionar todo

journal.window.entities.entity.delete.copy.select.select = seleccionar

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in = las transcripciones, los resúmenes y las actividades en otras partes de tu diario no se borran y pueden seguir mencionándolas.

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in.variant2 = las transcripciones, los resúmenes y las actividades en otras partes de tu diario siguen donde están y pueden seguir mencionándolas.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in = las transcripciones, los resúmenes y las actividades en otras partes de tu diario no se borran y pueden seguir mencionándola.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in.variant2 = las transcripciones, los resúmenes y las actividades en otras partes de tu diario siguen donde están y pueden seguir mencionándola.

journal.window.entities.entity.delete.copy.voice.unknown.what.your.journal.keeps.about.its = lo que tu diario guarda sobre su voz, si hay algo

journal.window.entities.entity.delete.copy.voice.what.your.journal.keeps.about.its = lo que tu diario guarda sobre su voz

journal.window.entities.examples.first.and.second = p. ej. { $first_name } y { $second_name }

journal.window.entities.examples.names = p. ej. { $names }

journal.window.entities.faces.chosen.alt = foto de perfil de { $entity_name }

journal.window.entities.faces.confirmed.lede = elegiste esta foto de perfil para { $entity_name }.

journal.window.entities.faces.dismissed.lede = no elegiste foto de perfil para { $entity_name }. si aparecen otros rostros que podrían ser { $entity_name }, pueden mostrarse aquí.

journal.window.entities.faces.done.chosen = foto de perfil guardada para { $entity_name }.

journal.window.entities.faces.done.dismissed = sin foto de perfil para { $entity_name }.

journal.window.entities.faces.done.rejected = ok, ese no es { $entity_name }.

journal.window.entities.faces.face.alt = un rostro de una reunión el { $day } que podría ser { $entity_name }

journal.window.entities.faces.facet.hint = elige una foto de perfil para { $entity_name }

journal.window.entities.faces.from.day = de una reunión el { $day }

journal.window.entities.faces.not.them = no es { $entity_name }

journal.window.entities.faces.possible.lede = estos rostros de tus reuniones podrían ser { $entity_name }. no se usa ninguno hasta que elijas uno.

journal.window.entities.faces.possible.title = elige una foto de perfil para { $entity_name }

journal.window.entities.faces.suggested.title = ¿es { $entity_name }?

journal.window.entities.faces.unconfirmed.alt = un rostro de una reunión que podría ser { $entity_name }, sin confirmar

journal.window.entities.facet.detach.confirm = ¿separar { $entity_name } de { $facet_name }? sigue en tu diario y puedes volver a agregarla cuando quieras.

journal.window.entities.facet.detach.done = se separó de { $facet_name }. { $entity_name } sigue en tu diario.

journal.window.entities.facet.empty.heading = todavía no hay nada en { $facet_name }.

journal.window.entities.facet.membership.list = en { $leading_facets } y { $final_facet }

journal.window.entities.facet.membership.single = en { $facet_name }

journal.window.entities.field.label.alternative.names.akas = nombres alternativos (AKAs)

journal.window.entities.field.label.description = descripción

journal.window.entities.field.label.name = nombre

journal.window.entities.field.label.type = tipo

journal.window.entities.field.placeholder.alias1.alias2.alias3 = alias1, alias2, alias3

journal.window.entities.field.placeholder.e.g.lead.engineer.on.the = p. ej., líder de ingeniería del proyecto, contacto principal de ventas…

journal.window.entities.field.placeholder.enter.description = escribe una descripción…

journal.window.entities.field.placeholder.search.entities = buscar entidades…

journal.window.entities.field.placeholder.type.a.name.to.find.it = escribe un nombre para encontrarlo

journal.window.entities.folder.all = todas

journal.window.entities.folder.companies = empresas

journal.window.entities.folder.duplicates = posibles duplicados

journal.window.entities.folder.hidden = ocultas

journal.window.entities.folder.other = otras

journal.window.entities.folder.people = personas

journal.window.entities.folder.projects = proyectos

journal.window.entities.folder.tools = herramientas

journal.window.entities.heading.couldn.t.load.entities = No se pudo cargar las entidades

journal.window.entities.heading.that.search.couldn.t.be.run = no se pudo ejecutar esa búsqueda

journal.window.entities.heading.this.entity.couldn.t.be.loaded = no se pudo cargar esta entidad

journal.window.entities.hide.already.absent.count =
    { $existing_quantity ->
        [one] 1 ya estaba oculta o ausente.
        [many] { $existing } ya estaban ocultas o ausentes.
       *[other] { $existing } ya estaban ocultas o ausentes.
    }

journal.window.entities.hide.detail.action.help = { $action_start }ocultar{ $action_end } { $description_start }la mueve a ocultas, donde puedes dejar de ocultarla. sus notas y sus detecciones en sus facetas salen de la búsqueda, y los agentes conectados no pueden alcanzarlas.{ $description_end }

journal.window.entities.hide.done.count =
    { $hidden_quantity ->
        [one] se ocultó 1 entidad.
        [many] se ocultaron { $hidden } entidades.
       *[other] se ocultaron { $hidden } entidades.
    }

journal.window.entities.hide.partial =
    { $remaining_quantity ->
        [one] se ocultó { $hidden_quantity ->
        [one] { $hidden }
        [many] { $hidden }
       *[other] { $hidden }
    } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. el resto no se ocultó.
        [many] se ocultó { $hidden_quantity ->
        [one] { $hidden }
        [many] { $hidden }
       *[other] { $hidden }
    } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. los demás no se ocultaron.
       *[other] se ocultó { $hidden_quantity ->
        [one] { $hidden }
        [many] { $hidden }
       *[other] { $hidden }
    } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. los demás no se ocultaron.
    }

journal.window.entities.hide.paused.records =
    { $records_quantity ->
        [one] la ocultación está en pausa: no se puede leer un registro de tu diario ({ $paths }).
        [many] la ocultación está en pausa: no se pueden leer { $records } registros de tu diario ({ $paths }).
       *[other] la ocultación está en pausa: no se pueden leer { $records } registros de tu diario ({ $paths }).
    }

journal.window.entities.hide.progress =
    { $done_quantity ->
        [one] ocultando… { $done } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
        [many] ocultando… { $done } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
       *[other] ocultando… { $done } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.entities.hide.record.error = no se pudo ocultarlas: no se puede leer un registro de tu diario ({ $record_detail }).

journal.window.entities.hide.selection.count =
    { $selected_quantity ->
        [one] ocultar { $selected }
        [many] ocultar { $selected }
       *[other] ocultar { $selected }
    }

journal.window.entities.hide.selection.progress =
    { $done_quantity ->
        [one] ocultando… { $done } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
        [many] ocultando… { $done } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
       *[other] ocultando… { $done } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.entities.hide.undo.done =
    { $restored_quantity ->
        [one] se devolvió 1 entidad.
        [many] se devolvieron { $restored } entidades.
       *[other] se devolvieron { $restored } entidades.
    }

journal.window.entities.hide.undo.help =
    { $hidden_quantity ->
        [one] deshacer la devuelve, facetas incluidas. también está en ocultas cuando quieras.
        [many] deshacer las devuelve, facetas incluidas. también están en ocultas cuando quieras.
       *[other] deshacer las devuelve, facetas incluidas. también están en ocultas cuando quieras.
    }

journal.window.entities.hide.undo.partial =
    { $failed_quantity ->
        [one] se devolvió { $restored_quantity ->
        [one] { $restored }
        [many] { $restored }
       *[other] { $restored }
    } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. no se pudo devolver 1; sigue en ocultas.
        [many] se devolvió { $restored_quantity ->
        [one] { $restored }
        [many] { $restored }
       *[other] { $restored }
    } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. no se pudieron devolver { $failed }; siguen en ocultas.
       *[other] se devolvió { $restored_quantity ->
        [one] { $restored }
        [many] { $restored }
       *[other] { $restored }
    } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. no se pudieron devolver { $failed }; siguen en ocultas.
    }

journal.window.entities.hide.undo.progress =
    { $restored_quantity ->
        [one] devolviendo… { $restored } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
        [many] devolviendo… { $restored } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
       *[other] devolviendo… { $restored } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }
    }

journal.window.entities.hide.undo.records.missing.one.entity =
    { $records_quantity ->
        [one] puede haber vuelto sin todas sus facetas: no se puede leer un registro de tu diario ({ $paths }).
        [many] puede haber vuelto sin todas sus facetas: no se pueden leer { $records } registros de tu diario ({ $paths }).
       *[other] puede haber vuelto sin todas sus facetas: no se pueden leer { $records } registros de tu diario ({ $paths }).
    }

journal.window.entities.hide.undo.records.missing.several.entities =
    { $records_quantity ->
        [one] algunas pueden haber vuelto sin todas sus facetas: no se puede leer un registro de tu diario ({ $paths }).
        [many] algunas pueden haber vuelto sin todas sus facetas: no se pueden leer { $records } registros de tu diario ({ $paths }).
       *[other] algunas pueden haber vuelto sin todas sus facetas: no se pueden leer { $records } registros de tu diario ({ $paths }).
    }

journal.window.entities.hide.undo.stopped =
    { $restored_quantity ->
        [one] deshacer se detuvo después de devolver { $restored } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. los demás siguen en ocultas.
        [many] deshacer se detuvo después de devolver { $restored } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. los demás siguen en ocultas.
       *[other] deshacer se detuvo después de devolver { $restored } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    }. los demás siguen en ocultas.
    }

journal.window.entities.link.label.back.to.entities = ← atrás a las entidades

journal.window.entities.link.label.edit = editar

journal.window.entities.link.label.in.all.facets = en todas las facetas →

journal.window.entities.list.heading.facet = entidades en { $facet_name }

journal.window.entities.list.last.mentioned.short.day = última mención { $day }

journal.window.entities.list.page.range = { $first }–{ $last } de { $total }

journal.window.entities.list.results.count =
    { $matched_quantity ->
        [one] { $matched } de { $total_quantity ->
        [one] { $total } entidad
        [many] { $total } entidades
       *[other] { $total } entidades
    }
        [many] { $matched } de { $total_quantity ->
        [one] { $total } entidad
        [many] { $total } entidades
       *[other] { $total } entidades
    }
       *[other] { $matched } de { $total_quantity ->
        [one] { $total } entidad
        [many] { $total } entidades
       *[other] { $total } entidades
    }
    }

journal.window.entities.list.select.all.count =
    { $total_quantity ->
        [one] seleccionar todo { $total }
        [many] seleccionar todo { $total }
       *[other] seleccionar todo { $total }
    }

journal.window.entities.list.selected.count =
    { $selected_quantity ->
        [one] { $selected } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } marcada
        [many] { $selected } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } marcadas
       *[other] { $selected } de { $total_quantity ->
        [one] { $total }
        [many] { $total }
       *[other] { $total }
    } marcadas
    }

journal.window.entities.list.sort.name = por nombre

journal.window.entities.list.sort.recent = reciente

journal.window.entities.list.sort.type = por tipo

journal.window.entities.merge.confirm.selection =
    { $selected_quantity ->
        [one] fusionar { $selected }
        [many] fusionar { $selected }
       *[other] fusionar { $selected }
    }

journal.window.entities.merge.evidence.meeting.photos = las fotos de reuniones sugieren que estas personas pueden ser la misma persona.

journal.window.entities.merge.keep.done =
    { $total_quantity ->
        [one] se mantuvo 1 par por separado.
        [many] se mantuvieron { $total } pares por separado.
       *[other] se mantuvieron { $total } pares por separado.
    }

journal.window.entities.merge.keep.partial =
    { $failed_quantity ->
        [one] se mantuvo { $processed_quantity ->
        [one] { $processed }
        [many] { $processed }
       *[other] { $processed }
    } de { $total_quantity ->
        [one] { $total } par
        [many] { $total } pares
       *[other] { $total } pares
    } por separado. 1 sigue en la lista.
        [many] se mantuvo { $processed_quantity ->
        [one] { $processed }
        [many] { $processed }
       *[other] { $processed }
    } de { $total_quantity ->
        [one] { $total } par
        [many] { $total } pares
       *[other] { $total } pares
    } por separado. { $failed } siguen en la lista.
       *[other] se mantuvo { $processed_quantity ->
        [one] { $processed }
        [many] { $processed }
       *[other] { $processed }
    } de { $total_quantity ->
        [one] { $total } par
        [many] { $total } pares
       *[other] { $total } pares
    } por separado. { $failed } siguen en la lista.
    }

journal.window.entities.merge.keep.selection =
    { $selected_quantity ->
        [one] mantener { $selected } por separado
        [many] mantener { $selected } por separado
       *[other] mantener { $selected } por separado
    }

journal.window.entities.merge.merge.done =
    { $total_quantity ->
        [one] se fusionó 1 par.
        [many] se fusionaron { $total } pares.
       *[other] se fusionaron { $total } pares.
    }

journal.window.entities.merge.merge.partial =
    { $failed_quantity ->
        [one] se fusionó { $processed_quantity ->
        [one] { $processed }
        [many] { $processed }
       *[other] { $processed }
    } de { $total_quantity ->
        [one] { $total } par
        [many] { $total } pares
       *[other] { $total } pares
    }. no se pudo fusionar 1.
        [many] se fusionó { $processed_quantity ->
        [one] { $processed }
        [many] { $processed }
       *[other] { $processed }
    } de { $total_quantity ->
        [one] { $total } par
        [many] { $total } pares
       *[other] { $total } pares
    }. no se pudieron fusionar { $failed }.
       *[other] se fusionó { $processed_quantity ->
        [one] { $processed }
        [many] { $processed }
       *[other] { $processed }
    } de { $total_quantity ->
        [one] { $total } par
        [many] { $total } pares
       *[other] { $total } pares
    }. no se pudieron fusionar { $failed }.
    }

journal.window.entities.merge.pair.confirm.aria =
    { $pairs_quantity ->
        [one] ¿fusionar 1 par?
        [many] ¿fusionar { $pairs } pares?
       *[other] ¿fusionar { $pairs } pares?
    }

journal.window.entities.merge.pair.confirm.heading =
    { $pairs_quantity ->
        [one] ¿fusionar 1 par?
        [many] ¿fusionar { $pairs } pares?
       *[other] ¿fusionar { $pairs } pares?
    }

journal.window.entities.merge.preview.direction = { $source_name } se fusiona en { $target_name }

journal.window.entities.merge.progress =
    { $done_quantity ->
        [one] fusionando… { $done } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
        [many] fusionando… { $done } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
       *[other] fusionando… { $done } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
    }

journal.window.entities.merge.progress.initial =
    { $total_quantity ->
        [one] fusionando… 0 de { $total }
        [many] fusionando… 0 de { $total }
       *[other] fusionando… 0 de { $total }
    }

journal.window.entities.merge.unmerged.pair.count =
    { $pairs_quantity ->
        [one] 1 par más sale de la lista sin fusionarse: incluye una entidad que ya se fusionó más arriba.
        [many] { $pairs } pares más salen de la lista sin fusionarse: cada uno incluye una entidad que ya se fusionó más arriba.
       *[other] { $pairs } pares más salen de la lista sin fusionarse: cada uno incluye una entidad que ya se fusionó más arriba.
    }

journal.window.entities.message.it.may.already.be.back.in = puede que ya haya vuelto a tu diario. recarga para revisar.

journal.window.entities.message.it.may.already.be.erased.reload = puede que ya se haya borrado. recarga para revisar.

journal.window.entities.message.it.may.already.be.in.the = puede que ya esté en la papelera. recarga para revisar.

journal.window.entities.message.it.s.back.in.your.journal = volvió a tu diario.

journal.window.entities.message.it.s.in.the.trash.you = está en la papelera. puedes volver a intentarlo desde ahí.

journal.window.entities.message.it.s.still.in.the.trash = sigue en la papelera. puedes volver a intentarlo desde ahí.

journal.window.entities.message.it.s.still.in.your.journal = sigue en tu diario.

journal.window.entities.message.they.may.already.be.back.in = puede que ya hayan vuelto a tu diario. recarga para revisar.

journal.window.entities.message.they.may.already.be.erased.reload = puede que ya se hayan borrado. recarga para revisar.

journal.window.entities.message.they.may.already.be.in.the = puede que ya estén en la papelera. recarga para revisar.

journal.window.entities.message.they.re.back.in.your.journal = volvieron a tu diario.

journal.window.entities.message.they.re.still.in.the.trash = siguen en la papelera. puedes volver a intentarlo desde ahí.

journal.window.entities.notes.count =
    { $notes_quantity ->
        [one] 1 nota
        [many] { $notes } notas
       *[other] { $notes } notas
    }

journal.window.entities.notes.none = sin notas

journal.window.entities.observation.coverage =
    { $observations_quantity ->
        [one] { $observations } observación a lo largo de { $days_quantity ->
            [one] { $days } día
            [many] { $days } días
           *[other] { $days } días
        }
        [many] { $observations } observaciones a lo largo de { $days_quantity ->
            [one] { $days } día
            [many] { $days } días
           *[other] { $days } días
        }
       *[other] { $observations } observaciones a lo largo de { $days_quantity ->
            [one] { $days } día
            [many] { $days } días
           *[other] { $days } días
        }
    }

journal.window.entities.observation.disclosure =
    { $visible_quantity ->
        [one] mostrando el { $visible } más reciente de { $total_quantity ->
            [one] { $total } momento
            [many] { $total } momentos
           *[other] { $total } momentos
        }
        [many] mostrando los { $visible } más recientes de { $total_quantity ->
            [one] { $total } momento
            [many] { $total } momentos
           *[other] { $total } momentos
        }
       *[other] mostrando los { $visible } más recientes de { $total_quantity ->
            [one] { $total } momento
            [many] { $total } momentos
           *[other] { $total } momentos
        }
    }

journal.window.entities.observation.grid.count =
    { $quantity ->
        [one] 1 momento
        [many] { $count } momentos
       *[other] { $count } momentos
    }

journal.window.entities.observation.latest.day = día más reciente: { $day }

journal.window.entities.observation.none.with.entity = todavía no hay nada con { $entity_name }.

journal.window.entities.observation.range = { $start_day } – { $end_day }

journal.window.entities.observation.show.all =
    { $observations_quantity ->
        [one] mostrar { $observations } observación
        [many] mostrar todas las { $observations } observaciones
       *[other] mostrar todas las { $observations } observaciones
    }

journal.window.entities.response.alias.conflict = El alias '{ $alias }' entra en conflicto con la entidad '{ $conflicting_name }'.

journal.window.entities.response.alias.conflict.rename = El alias '{ $alias }' entra en conflicto con la entidad '{ $conflicting_name }'

journal.window.entities.response.ambiguity.dismiss.status = no se puede descartar la ambigüedad con estado { $ambiguity_status }

journal.window.entities.response.ambiguity.file.corrupt = el archivo de ambigüedad { $file_path } contiene una fila corrupta

journal.window.entities.response.candidate.facet.already.exists = La faceta '{ $facet }' ya existe

journal.window.entities.response.candidate.facet.candidate.accept.failed = no se pudo aceptar el candidato

journal.window.entities.response.candidate.facet.candidate.dismiss.failed = no se pudo descartar el candidato

journal.window.entities.response.candidate.facet.candidate.read.failed = no se pudo leer el candidato

journal.window.entities.response.candidate.facet.facet.creation.failed = no se pudo crear la faceta

journal.window.entities.response.candidate.facet.facet.lookup.failed = no se pudo buscar la faceta

journal.window.entities.response.candidate.facet.slug.invalid = Nombre de faceta no válido '{ $facet }': debe estar en minúsculas, empezar con una letra y contener solo letras, dígitos, guiones o guiones bajos

journal.window.entities.response.candidate.not.found = no se encontró el candidato

journal.window.entities.response.candidate.record.missing = el candidato de fusión no tiene un registro activo que coincida

journal.window.entities.response.candidate.status.accept = no se puede aceptar el candidato con estado{ $slot1 }{ $status }

journal.window.entities.response.candidate.status.dismiss = no se puede descartar el candidato con estado{ $slot1 }{ $status }

journal.window.entities.response.candidate.status.preview = no se puede previsualizar el candidato con estado{ $slot1 }{ $status }

journal.window.entities.response.delete.refusal.ambiguities.unreadable = no se pudieron leer los registros de ambigüedades de tu diario

journal.window.entities.response.delete.refusal.bad.request = no se pudo leer esa eliminación

journal.window.entities.response.delete.refusal.busy = tu diario está ocupado; vuelve a intentarlo en un momento

journal.window.entities.response.delete.refusal.failed = tu diario no pudo moverla a la papelera

journal.window.entities.response.delete.refusal.gone = esa entidad no está en tu diario

journal.window.entities.response.delete.refusal.link.unreadable = no se pudo leer un enlace de faceta

journal.window.entities.response.delete.refusal.principal = no puedes eliminarte

journal.window.entities.response.delete.refusal.record.damaged = no se pudo actualizar el registro de entidades eliminadas de tu diario

journal.window.entities.response.detail.access.denied = acceso denegado

journal.window.entities.response.detail.after.id.and.offset.cannot.be.used.together = { $slot1 } y el desplazamiento no se pueden usar juntos

journal.window.entities.response.detail.after.id.must.be.a.non.negative.integer = { $slot1 } debe ser un entero no negativo

journal.window.entities.response.detail.aka.is.required = el alias es obligatorio

journal.window.entities.response.detail.ambiguity.dismiss.failed = no se pudo descartar la ambigüedad

journal.window.entities.response.detail.ambiguity.id.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.ambiguity.not.found = no se encontró la ambigüedad

journal.window.entities.response.detail.ambiguity.read.failed = no se pudo leer la ambigüedad

journal.window.entities.response.detail.ambiguity.resolve.failed = no se pudo resolver la ambigüedad

journal.window.entities.response.detail.batch.block.failed = no se pudo bloquear el lote

journal.window.entities.response.detail.batch.unblock.failed = no se pudo desbloquear el lote

journal.window.entities.response.detail.body.is.not.valid.json = el cuerpo no es un { $slot1 } válido

journal.window.entities.response.detail.body.must.be.a.json.object = el cuerpo debe ser un objeto { $slot1 }

journal.window.entities.response.detail.candidate.read.failed = no se pudo leer el candidato

journal.window.entities.response.detail.content.is.required = el contenido es obligatorio

journal.window.entities.response.detail.couldn.t.hide.entity = no se pudo ocultar la entidad

journal.window.entities.response.detail.couldn.t.unhide.entity = no se pudo dejar de ocultar la entidad

journal.window.entities.response.detail.day.is.required = el día es obligatorio

journal.window.entities.response.detail.description.is.required = la descripción es obligatoria

journal.window.entities.response.detail.detected.entity.activity.is.unavailable = la actividad de la entidad detectada no está disponible

journal.window.entities.response.detail.detected.entity.not.found = no se encontró la entidad detectada

journal.window.entities.response.detail.detected.entity.save.failed = no se pudo guardar la entidad detectada

journal.window.entities.response.detail.detected.entity.update.failed = no se pudo actualizar la entidad detectada

journal.window.entities.response.detail.detected.read.failed = no se pudo leer lo detectado

journal.window.entities.response.detail.detections.can.t.be.shown.here.which.names.are = no se pueden mostrar las detecciones aquí: no se puede leer qué nombres están ocultos

journal.window.entities.response.detail.entity.alias.update.failed = no se pudo actualizar el alias de la entidad

journal.window.entities.response.detail.entity.already.exists = la entidad ya existe

journal.window.entities.response.detail.entity.already.exists.in.destination.facet.use.merge.to = La entidad ya existe en la faceta de destino. Usa { $slot1 } para fusionar.

journal.window.entities.response.detail.entity.busy = entidad ocupada

journal.window.entities.response.detail.entity.create.failed = no se pudo crear la entidad

journal.window.entities.response.detail.entity.data.directory.not.found.in.source.facet = No se encontró el directorio de datos de la entidad en la faceta de origen.

journal.window.entities.response.detail.entity.description.update.failed = no se pudo actualizar la descripción de la entidad

journal.window.entities.response.detail.entity.detach.failed = no se pudo separar la entidad

journal.window.entities.response.detail.entity.hidden = entidad oculta

journal.window.entities.response.detail.entity.id.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.entity.is.required = la entidad es obligatoria

journal.window.entities.response.detail.entity.move.failed = no se pudo mover la entidad

journal.window.entities.response.detail.entity.name.is.invalid = el nombre de la entidad no es válido

journal.window.entities.response.detail.entity.not.found = no se encontró la entidad

journal.window.entities.response.detail.entity.not.found.in.facet = No se encontró la entidad en la faceta

journal.window.entities.response.detail.entity.not.found.variant2 = No se encontró la entidad

journal.window.entities.response.detail.entity.read.failed = no se pudo leer la entidad

journal.window.entities.response.detail.entity.restore.failed = no se pudo restaurar la entidad

journal.window.entities.response.detail.entity.search.index.is.busy = el índice de búsqueda de entidades está ocupado

journal.window.entities.response.detail.entity.search.index.is.stale = el índice de búsqueda de entidades está desactualizado

journal.window.entities.response.detail.entity.search.index.is.unavailable = el índice de búsqueda de entidades no está disponible

journal.window.entities.response.detail.entity.update.failed = no se pudo actualizar la entidad

journal.window.entities.response.detail.entity.with.this.name.already.exists.in.facet = Ya existe una entidad con este nombre en la faceta

journal.window.entities.response.detail.evidence.is.required = la evidencia es obligatoria

journal.window.entities.response.detail.exclude.name.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.facet.is.required = la faceta es obligatoria

journal.window.entities.response.detail.facet.not.found = no se encontró la faceta

journal.window.entities.response.detail.facet.read.failed = no se pudo leer la faceta

journal.window.entities.response.detail.from.facet.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.grid.read.failed = no se pudo leer la cuadrícula

journal.window.entities.response.detail.group.resolve.failed = no se pudo resolver el grupo

journal.window.entities.response.detail.hidden.must.be.include.exclude.or.only = { $slot1 } debe ser { $slot2 }, { $slot3 } o { $slot4 }

journal.window.entities.response.detail.history.read.failed = no se pudo leer el historial

journal.window.entities.response.detail.history.version.was.not.found = no se encontró la versión del historial

journal.window.entities.response.detail.ids.elements.must.be.strings = los elementos de ids deben ser cadenas

journal.window.entities.response.detail.ids.is.required = ids es obligatorio

journal.window.entities.response.detail.ids.must.be.an.array = ids debe ser un arreglo

journal.window.entities.response.detail.index.plate.task.failed = falló la tarea de la placa de índice

journal.window.entities.response.detail.invalid.detected.entity = entidad detectada no válida

journal.window.entities.response.detail.invalid.target.kind = clase de destino no válida

journal.window.entities.response.detail.journal.read.failed = no se pudo leer el diario

journal.window.entities.response.detail.limit.bounded.two.hundred = el límite debe estar entre 0 y 200

journal.window.entities.response.detail.limit.is.required = el límite es obligatorio

journal.window.entities.response.detail.limit.must.be.an.integer = el límite debe ser un entero

journal.window.entities.response.detail.member.ids.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.member.ids.must.contain.only.strings = { $slot1 } debe contener solo cadenas

journal.window.entities.response.detail.member.ids.must.not.be.empty = { $slot1 } no debe estar vacío

journal.window.entities.response.detail.merge.candidate.accept.failed = no se pudo aceptar el candidato de fusión

journal.window.entities.response.detail.merge.candidate.dismiss.failed = no se pudo descartar el candidato de fusión

journal.window.entities.response.detail.merge.candidate.record.failed = no se pudo registrar el candidato de fusión

journal.window.entities.response.detail.merge.commit.failed = no se pudo aplicar la fusión

journal.window.entities.response.detail.merge.preview.failed = no se pudo previsualizar la fusión

journal.window.entities.response.detail.missing.name.key = Falta { $slot1 }

journal.window.entities.response.detail.name.is.required = el nombre es obligatorio

journal.window.entities.response.detail.no.data.provided = No se proporcionaron datos

journal.window.entities.response.detail.observation.content.cannot.be.empty = el contenido de la observación no puede estar vacío

journal.window.entities.response.detail.observation.entity.lookup.failed = no se pudo buscar la entidad de la observación

journal.window.entities.response.detail.offset.must.be.a.non.negative.integer = el desplazamiento debe ser un entero no negativo

journal.window.entities.response.detail.offset.must.not.be.negative = el desplazamiento no debe ser negativo

journal.window.entities.response.detail.old.name.and.new.name.are.required = { $slot1 } y { $slot2 } son obligatorios

journal.window.entities.response.detail.operation.is.required = la operación es obligatoria

journal.window.entities.response.detail.operation.must.be.a.string = la operación debe ser una cadena

journal.window.entities.response.detail.recorded.entity.choice.is.absent.from.this.facet = la elección de entidad registrada está ausente de esta faceta

journal.window.entities.response.detail.recorded.entity.choice.is.hidden = la elección de entidad registrada está oculta

journal.window.entities.response.detail.request.body.could.not.be.read = no se pudo leer el cuerpo de la solicitud

journal.window.entities.response.detail.resolution.task.failed = falló la tarea de resolución

journal.window.entities.response.detail.restore.is.not.available = la restauración no está disponible

journal.window.entities.response.detail.review.item.not.found = no se encontró el elemento de revisión

journal.window.entities.response.detail.review.read.failed = no se pudo leer la revisión

journal.window.entities.response.detail.review.restore.failed = no se pudo restaurar la revisión

journal.window.entities.response.detail.review.sweep.failed = falló el barrido de revisión

journal.window.entities.response.detail.revision.is.required = la revisión es obligatoria

journal.window.entities.response.detail.since.must.be.a.yyyymmdd.day = desde debe ser un día { $slot1 }

journal.window.entities.response.detail.sort.must.be.activity.or.name = { $slot1 } debe ser { $slot2 } o { $slot3 }

journal.window.entities.response.detail.source.and.target.resolve.to.the.same.entity = la fuente y el destino corresponden a la misma entidad.

journal.window.entities.response.detail.source.is.required = la fuente es obligatoria

journal.window.entities.response.detail.source.slug.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.status.must.be.open.or.resolved = { $slot1 } debe ser { $slot2 } o { $slot3 }

journal.window.entities.response.detail.suggestions.are.busy.try.again = las sugerencias están ocupadas; vuelve a intentarlo

journal.window.entities.response.detail.target.is.required = el destino es obligatorio

journal.window.entities.response.detail.target.slug.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.the.name.couldn.t.be.changed.so.nothing.changed = no se pudo cambiar el nombre, así que no cambió nada

journal.window.entities.response.detail.the.names.hidden.in.this.facet.can.t.be = no se pueden leer los nombres ocultos en esta faceta, así que no cambió nada

journal.window.entities.response.detail.this.entity.is.hidden.so.its.names.can.t = esta entidad está oculta, así que sus nombres no pueden cambiar hasta que se deje de ocultar

journal.window.entities.response.detail.this.entity.is.hidden.unhide.it.in.the.entities = Esta entidad está oculta. Deja de ocultarla en la app de entidades primero.

journal.window.entities.response.detail.to.facet.is.required = { $slot1 } es obligatorio

journal.window.entities.response.detail.type.and.name.are.required = El tipo y el nombre son obligatorios

journal.window.entities.response.detail.type.is.required = el tipo es obligatorio

journal.window.entities.response.detail.type.must.be.a.canonical.entity.type.or.other = el tipo debe ser un tipo de entidad canónico u otro

journal.window.entities.response.detail.version.id.is.required = { $slot1 } es obligatorio

journal.window.entities.response.entity.exists = La entidad '{ $entity_name }' ya existe

journal.window.entities.response.entity.merged.permanent = '{ $merged_id }' se fusionó en '{ $survivor_id }'. Las fusiones no se pueden deshacer, así que ese nombre no se puede agregar como entidad nueva; usa '{ $survivor_id }' en su lugar.

journal.window.entities.response.entity.missing = No se encontró la entidad '{ $entity_id }'

journal.window.entities.response.entity.type.invalid = Tipo de entidad no válido { $entity_type }

journal.window.entities.response.entity.type.invalid.query = Tipo de entidad no válido: { $entity_type }

journal.window.entities.response.entity.type.invalid.write = Tipo de entidad no válido '{ $entity_type }'

journal.window.entities.response.facet.entity.missing = '{ $entity_name }' no está en esta faceta. agrégala aquí primero y luego agrega notas sobre ella.

journal.window.entities.response.ids.count.invalid = número de ids no válido: { $id_count }

journal.window.entities.response.merge.refusal.both.principal = no se pueden fusionar estas: ambas están marcadas como tú.

journal.window.entities.response.merge.refusal.person.type = no se puede fusionar una persona en algo que no es una persona.

journal.window.entities.response.merge.refusal.principal.type = no se te puede fusionar en algo que no es una persona.

journal.window.entities.response.merge.refusal.same.entity = La fuente y el destino deben ser entidades distintas.

journal.window.entities.response.merge.refusal.source.hidden = no se puede fusionar algo que está oculto. deja de ocultarlo primero.

journal.window.entities.response.merge.refusal.source.missing = No se encontró la entidad de origen:{ $slot1 }{ $id }

journal.window.entities.response.merge.refusal.target.hidden = no se puede fusionar en algo que está oculto. deja de ocultarlo primero.

journal.window.entities.response.merge.refusal.target.missing = No se encontró la entidad de destino:{ $slot1 }{ $id }

journal.window.entities.response.merge.repair = Contacta a un operador para reparar esta fusión.

journal.window.entities.response.operation.already.undone = la operación { $operation_id } ya se deshizo

journal.window.entities.response.operation.id.invalid = id de operación no válido: { $operation_id }

journal.window.entities.response.operation.limit.invalid = límite no válido: { $limit }

journal.window.entities.response.photo.entity.not.found = no se pudo encontrar a esa persona.

journal.window.entities.response.photo.face.action.changed = esto cambió desde que se cargó la página. recarga la página.

journal.window.entities.response.photo.face.not.found = esa foto ya no está disponible.

journal.window.entities.response.photo.faces.busy = los datos del rostro están ocupados. vuelve a intentarlo pronto.

journal.window.entities.response.photo.faces.disabled = activa las sugerencias de foto de perfil antes de cambiar una foto de perfil.

journal.window.entities.response.photo.faces.enabled = desactiva las sugerencias de foto de perfil antes de borrar los datos del rostro.

journal.window.entities.response.photo.faces.read.failed = no se pudieron leer esas fotos. vuelve a intentarlo.

journal.window.entities.response.photo.request.invalid = no se pudo leer esa solicitud de foto.

journal.window.entities.response.principal.protected = la entidad principal está protegida de esta operación: { $entity_id }

journal.window.entities.response.query.integer.bounded = { $parameter_name } debe estar entre 0 y { $maximum }

journal.window.entities.response.query.integer.required = { $parameter_name } debe ser un número entero

journal.window.entities.response.refused = Solicitud de entidad rechazada

journal.window.entities.response.summary.limit.bounded = el límite debe estar entre 0 y { $maximum }

journal.window.entities.scope.showing.facet = mostrando { $facet_name } ⌄

journal.window.entities.scope.showing.journal = mostrando todo tu diario ⌄

journal.window.entities.search.finding.names = buscando entidades que coincidan con "{ $query }"…

journal.window.entities.search.finding.type = buscando entidades de tipo { $entity_type }…

journal.window.entities.search.finding.type.and.names = buscando entidades de tipo { $entity_type } que coincidan con "{ $query }"…

journal.window.entities.search.none.match = ninguna entidad coincide con "{ $query }".

journal.window.entities.search.placeholder.entity.count =
    { $total_quantity ->
        [one] buscar { $total } entidad
        [many] buscar { $total } entidades
       *[other] buscar { $total } entidades
    }

journal.window.entities.section.heading.detach = separar

journal.window.entities.section.heading.no.entities.yet = todavía no hay entidades

journal.window.entities.state.edit.loading = cargando…

journal.window.entities.state.edit.unavailable = no disponible

journal.window.entities.text.couldn.t.hide.them = no se pudieron ocultar.

journal.window.entities.text.hiding.stopped.partway.undo.puts.back = la ocultación se detuvo a medias. deshacer devuelve todo lo que ocultó.

journal.window.entities.text.loading = cargando…

journal.window.entities.text.loading.entities = cargando entidades…

journal.window.entities.text.undo.stopped.before.putting.any.back = deshacer se detuvo antes de devolver alguna. siguen en ocultas.

journal.window.entities.tidy.duplicate.count =
    { $pairs_quantity ->
        [one] { $pairs } posible duplicado
        [many] { $pairs } posibles duplicados
       *[other] { $pairs } posibles duplicados
    }

journal.window.entities.tidy.group.load.error = no se pudo cargar este grupo.

journal.window.entities.tidy.id.count =
    { $entities_quantity ->
        [one] { $entities } parece un ID
        [many] { $entities } parecen un ID
       *[other] { $entities } parecen un ID
    }

journal.window.entities.title.couldn.t.erase.those = no se pudieron borrar esos

journal.window.entities.title.couldn.t.move.that.to.the = no se pudo mover eso a la papelera

journal.window.entities.title.couldn.t.move.those.to.the = no se pudieron mover esos a la papelera

journal.window.entities.title.couldn.t.recover.those = no se pudieron recuperar esos

journal.window.entities.title.delete.this.entity = ¿eliminar esta entidad?

journal.window.entities.title.erase.this.entity.now = ¿borrar esta entidad ahora?

journal.window.entities.title.moved.to.the.trash = se movió a la papelera

journal.window.entities.title.your.journal.didn.t.answer = tu diario no respondió

journal.window.entities.tooltip.add.to.facet = Agregar a la faceta

journal.window.entities.tooltip.cannot.hide.yourself = no puedes ocultarte

journal.window.entities.tooltip.entity.is.hidden.and.detached.from = la entidad está oculta y separada de esta faceta

journal.window.entities.tooltip.entity.type = tipo: { $entity_type }

journal.window.entities.tooltip.facet.name = faceta: { $facet_name }

journal.window.entities.tooltip.has.voiceprint = Tiene huella de voz

journal.window.entities.tooltip.last.mentioned.full.day = Última mención: { $day }

journal.window.entities.tooltip.last.mentioned.short.day = última mención { $day }

journal.window.entities.tooltip.mention.days =
    { $mentions_quantity ->
        [one] { $mentions } mención: días detectados en los últimos 30 días
        [many] { $mentions } menciones: días detectados en los últimos 30 días
       *[other] { $mentions } menciones: días detectados en los últimos 30 días
    }

journal.window.entities.tooltip.this.entity.is.hidden = esta entidad está oculta

journal.window.entities.tooltip.this.is.you = eres tú

journal.window.entities.tooltip.this.is.your.entity.in.this = Esta es tu entidad en esta faceta

journal.window.entities.tooltip.unhide.this.entity = dejar de ocultar esta entidad

journal.window.entities.tooltip.view.in.facet = Haz clic para ver en { $facet_name }

journal.window.entities.trust.history.change = { $previous_identity } → { $updated_identity }

journal.window.entities.trust.history.event = { $event_kind } · { $event_time } · { $entity_name }

journal.window.entities.trust.history.suggested.by = sugerido por { $agent_name }

journal.window.entities.trust.identity.details = { $entity_name } · { $entity_type } · alias: { $aliases }

journal.window.entities.trust.merge.confirm = ¿fusionar { $source_name } en { $target_name }? una fusión no se puede deshacer.

journal.window.entities.trust.merge.preview.counts =
    { $voice_samples_quantity ->
        [one] muestras de voz agregadas: { $voice_samples } · muestras después de fusionar: { $voice_total_quantity ->
            [one] { $voice_total }
            [many] { $voice_total }
           *[other] { $voice_total }
        } · notas bajo el nombre que se fusiona: { $notes_quantity ->
            [one] { $notes }
            [many] { $notes }
           *[other] { $notes }
        } · etiquetas de hablante cambiadas: { $speaker_labels_quantity ->
            [one] { $speaker_labels }
            [many] { $speaker_labels }
           *[other] { $speaker_labels }
        } · registros de actividad cambiados: { $activities_quantity ->
            [one] { $activities }
            [many] { $activities }
           *[other] { $activities }
        }
        [many] muestras de voz agregadas: { $voice_samples } · muestras después de fusionar: { $voice_total_quantity ->
            [one] { $voice_total }
            [many] { $voice_total }
           *[other] { $voice_total }
        } · notas bajo el nombre que se fusiona: { $notes_quantity ->
            [one] { $notes }
            [many] { $notes }
           *[other] { $notes }
        } · etiquetas de hablante cambiadas: { $speaker_labels_quantity ->
            [one] { $speaker_labels }
            [many] { $speaker_labels }
           *[other] { $speaker_labels }
        } · registros de actividad cambiados: { $activities_quantity ->
            [one] { $activities }
            [many] { $activities }
           *[other] { $activities }
        }
       *[other] muestras de voz agregadas: { $voice_samples } · muestras después de fusionar: { $voice_total_quantity ->
            [one] { $voice_total }
            [many] { $voice_total }
           *[other] { $voice_total }
        } · notas bajo el nombre que se fusiona: { $notes_quantity ->
            [one] { $notes }
            [many] { $notes }
           *[other] { $notes }
        } · etiquetas de hablante cambiadas: { $speaker_labels_quantity ->
            [one] { $speaker_labels }
            [many] { $speaker_labels }
           *[other] { $speaker_labels }
        } · registros de actividad cambiados: { $activities_quantity ->
            [one] { $activities }
            [many] { $activities }
           *[other] { $activities }
        }
    }

journal.window.entities.trust.merge.preview.summary =
    { $aliases_quantity ->
        [one] alias agregados: { $aliases } · correos electrónicos agregados: { $emails_quantity ->
            [one] { $emails }
            [many] { $emails }
           *[other] { $emails }
        }
        [many] alias agregados: { $aliases } · correos electrónicos agregados: { $emails_quantity ->
            [one] { $emails }
            [many] { $emails }
           *[other] { $emails }
        }
       *[other] alias agregados: { $aliases } · correos electrónicos agregados: { $emails_quantity ->
            [one] { $emails }
            [many] { $emails }
           *[other] { $emails }
        }
    }

journal.window.entities.trust.suggestion.details = { $source_name } → { $target_name } · { $facet_name }

journal.window.entities.visible.text.attach.to = adjuntar a

journal.window.entities.visible.text.attach.to.facet = adjuntar a la faceta

journal.window.entities.visible.text.attaching = adjuntando…

journal.window.entities.visible.text.cancel = cancelar

journal.window.entities.visible.text.clear.selection = limpiar la selección

journal.window.entities.visible.text.comma.separated.list.of.alternative.names = lista separada por comas de nombres alternativos para la transcripción de audio

journal.window.entities.visible.text.connections = conexiones

journal.window.entities.visible.text.couldn.t.load.that.page.of = no se pudo cargar esa página de entidades.

journal.window.entities.visible.text.description = descripción

journal.window.entities.visible.text.detach = separar

journal.window.entities.visible.text.detached = separada

journal.window.entities.visible.text.detected.entities = entidades detectadas

journal.window.entities.visible.text.dots.are.the.facets.a.name = los puntos son las facetas en las que aparece un nombre.

journal.window.entities.visible.text.each.pair.below.becomes.one.entity = cada par de abajo se convierte en una entidad. fusionar no se puede deshacer.

journal.window.entities.visible.text.edit.entity = editar entidad

journal.window.entities.visible.text.entities = entidades

journal.window.entities.visible.text.facets = facetas

journal.window.entities.visible.text.failed.to.save.changes = no se pudieron guardar los cambios

journal.window.entities.visible.text.found.by.ai.in.the.last = encontradas por IA en los últimos 30 días. pon una estrella para agregarlas a tu diario

journal.window.entities.visible.text.has.voiceprint = Tiene huella de voz

journal.window.entities.visible.text.hidden = oculta

journal.window.entities.visible.text.hide = ocultar

journal.window.entities.visible.text.hiding = ocultando…

journal.window.entities.visible.text.how.does.this.entity.relate.to = (¿cómo se relaciona esta entidad con esta faceta?)

journal.window.entities.visible.text.loading = cargando…

journal.window.entities.visible.text.loading.entities = cargando entidades…

journal.window.entities.visible.text.loading.this.entity = cargando esta entidad…

journal.window.entities.visible.text.more.about.hidden = más sobre lo oculto

journal.window.entities.visible.text.name.is.required = el nombre es obligatorio

journal.window.entities.visible.text.names.shaped.like.codes.file.names = nombres con forma de códigos, nombres de archivo o números de solicitud. ninguno tiene una nota.

journal.window.entities.visible.text.no.description.click.to.add = sin descripción. haz clic para agregar…

journal.window.entities.visible.text.no.entities.match.your.search = ninguna entidad coincide con tu búsqueda.

journal.window.entities.visible.text.no.entity.matches.that.name = ninguna entidad coincide con ese nombre.

journal.window.entities.visible.text.not.attached = sin adjuntar

journal.window.entities.visible.text.not.attached.to.any.facets.yet = todavía sin adjuntar a ninguna faceta.

journal.window.entities.visible.text.not.now = ahora no

journal.window.entities.visible.text.nothing.here = aquí no hay nada.

journal.window.entities.visible.text.only.in.facets.you.muted = solo en facetas que silenciaste.

journal.window.entities.visible.text.pairs.that.might.be.the.same = pares que podrían ser la misma persona o la misma cosa. fusionar no se puede deshacer.

journal.window.entities.visible.text.people.places.and.things.you.re = personas, lugares y cosas que cuidas

journal.window.entities.visible.text.review = revisar

journal.window.entities.visible.text.review.the.group = revisar el grupo

journal.window.entities.visible.text.save = guardar

journal.window.entities.visible.text.save.changes = guardar cambios

journal.window.entities.visible.text.saved = ¡se guardó!

journal.window.entities.visible.text.saving = guardando…

journal.window.entities.visible.text.select.a.facet.to.add.entities = Selecciona una faceta para agregar entidades a tu diario.

journal.window.entities.visible.text.still.working = todavía trabajando…

journal.window.entities.visible.text.they.move.to.hidden.where.you = se mueven a ocultas, donde puedes dejar de ocultarlas. los agentes conectados no pueden conectar con ellas. tu propia entidad nunca está en este grupo.

journal.window.entities.visible.text.try.again = reintentar

journal.window.entities.visible.text.undo = deshacer

journal.window.entities.visible.text.unhide = dejar de ocultar

journal.window.entities.visible.text.unhiding = dejando de ocultar…

journal.window.entities.visible.text.unticked.ones.stay.as.they.are = las que no están marcadas se quedan como están.

journal.window.entities.visible.text.worth.a.look = merece un vistazo

journal.window.entities.visible.text.you = tú

journal.window.entities.voice.heading.count =
    { $samples_quantity ->
        [one] muestras de voz ({ $samples })
        [many] muestras de voz ({ $samples })
       *[other] muestras de voz ({ $samples })
    }

journal.window.entities.voice.invalid.key = clave de muestra de voz no válida

journal.window.entities.voice.sample.sentence = oración { $sentence_id }

journal.window.entities.window.explanation.couldn.t.keep.it.separate.it = no se pudo mantener por separado. sigue en la lista.

journal.window.entities.window.explanation.couldn.t.keep.them.separate.they = no se pudieron mantener por separado. siguen en la lista.

journal.window.entities.window.explanation.couldn.t.merge.it.it.s = no se pudo fusionar. sigue en la lista.

journal.window.entities.window.explanation.couldn.t.merge.them.they.re = no se pudieron fusionar. siguen en la lista.

journal.window.entities.window.explanation.couldn.t.search.for.a.merge = no se pudo buscar un destino para fusionar.

journal.window.entities.window.explanation.failed.to.load.observations = no se pudieron cargar las observaciones

journal.window.entities.window.explanation.look.like.an.id = parecen un ID

journal.window.entities.window.explanation.this.entity.couldn.t.be.loaded = no se pudo cargar esta entidad, así que no se puede editar.

journal.window.entities.window.state.text.couldn.t.attach.entity = no se pudo adjuntar la entidad

journal.window.entities.window.state.text.couldn.t.detach.entity = no se pudo separar la entidad

journal.window.entities.window.state.text.couldn.t.hide.entity = no se pudo ocultar la entidad

journal.window.entities.window.state.text.couldn.t.re.attach.entity = no se pudo volver a agregar la entidad

journal.window.entities.window.state.text.couldn.t.save.description = no se pudo guardar la descripción

journal.window.entities.window.state.text.couldn.t.unhide.entity = no se pudo dejar de ocultar la entidad

journal.window.entities.window.state.text.failed.to.attach = No se pudo adjuntar

journal.window.entities.window.state.text.failed.to.attach.variant2 = no se pudo adjuntar

journal.window.entities.window.state.text.failed.to.detach = No se pudo separar

journal.window.entities.window.state.text.failed.to.re.attach = No se pudo volver a agregar

journal.window.entities.window.state.text.failed.to.save = No se pudo guardar

journal.window.entities.window.state.text.finding.entities = buscando entidades…

journal.window.entities.window.state.text.loading.this.entity = cargando esta entidad…

journal.window.entities.window.state.text.next.page = página siguiente

journal.window.entities.window.state.text.not.set = sin definir

journal.window.entities.window.state.text.previous.page = página anterior

journal.window.entities.window.state.text.suggested.match = coincidencia sugerida

journal.window.entities.window.state.text.try.again = vuelve a intentarlo.

journal.window.facets.activities.calendar.scheduled.title = en tu calendario en ese momento: { $meeting_title }

journal.window.facets.activities.copy.action.correct = eso no está bien

journal.window.facets.activities.copy.action.edit = editar

journal.window.facets.activities.copy.action.hide = ocultar

journal.window.facets.activities.copy.action.separate = separar

journal.window.facets.activities.copy.action.separate.yes = separarla

journal.window.facets.activities.copy.action.unhide = dejar de ocultar

journal.window.facets.activities.copy.back = atrás

journal.window.facets.activities.copy.cancel = cancelar

journal.window.facets.activities.copy.correct.title = eso no está bien

journal.window.facets.activities.copy.correct.what = qué pasó en realidad

journal.window.facets.activities.copy.correct.who = quién lo dijo

journal.window.facets.activities.copy.corrected.by.you = la corregiste tú

journal.window.facets.activities.copy.cue.edited = la editaste tú

journal.window.facets.activities.copy.empty.facet.show.all = mostrar todas las facetas

journal.window.facets.activities.copy.empty.today.heading = todavía no hay nada hoy

journal.window.facets.activities.copy.evidence.open = abrir en transcripciones ›

journal.window.facets.activities.copy.evidence.open.narrow = abrir ›

journal.window.facets.activities.copy.heading.accounts = qué lo registró

journal.window.facets.activities.copy.heading.description = descripción

journal.window.facets.activities.copy.heading.history = historial

journal.window.facets.activities.copy.heading.summary = resumen

journal.window.facets.activities.copy.hidden.outline = oculta

journal.window.facets.activities.copy.history.remove = quitar

journal.window.facets.activities.copy.history.removed = quitada

journal.window.facets.activities.copy.history.you.hid = la ocultaste

journal.window.facets.activities.copy.history.you.unhid = dejaste de ocultarla

journal.window.facets.activities.copy.home.today.link = todas las actividades de hoy →

journal.window.facets.activities.copy.label.description = descripción

journal.window.facets.activities.copy.label.title = título

journal.window.facets.activities.copy.lines.loading = buscando las líneas habladas…

journal.window.facets.activities.copy.lines.only = solo estas líneas

journal.window.facets.activities.copy.lines.others = otras líneas habladas

journal.window.facets.activities.copy.load.error = no se pudieron cargar las actividades

journal.window.facets.activities.copy.loading = cargando actividades…

journal.window.facets.activities.copy.now = ahora

journal.window.facets.activities.copy.people.loading = cargando las personas de tu diario…

journal.window.facets.activities.copy.person.search = encontrar una persona

journal.window.facets.activities.copy.reason.other = oculta

journal.window.facets.activities.copy.reason.owner = ocultaste esto

journal.window.facets.activities.copy.save = guardar

journal.window.facets.activities.copy.scope.all = mostrando todas las facetas

journal.window.facets.activities.copy.scope.all.option = todas las facetas

journal.window.facets.activities.copy.sheet.close = cerrar

journal.window.facets.activities.copy.show.hidden = mostrar ocultas

journal.window.facets.activities.copy.source.unknown = fuente desconocida

journal.window.facets.activities.copy.speaker.me = yo

journal.window.facets.activities.copy.speaker.person = una persona

journal.window.facets.activities.copy.state.removed = quitada

journal.window.facets.activities.copy.state.removed.by.you = la quitaste tú

journal.window.facets.activities.copy.stop.showing.hidden = dejar de mostrar ocultas

journal.window.facets.activities.copy.unhide = dejar de ocultar

journal.window.facets.activities.copy.untimed.heading = sin hora

journal.window.facets.activities.copy.updating = actualizando…

journal.window.facets.activities.empty.facet.day = nada en { $facet_name } en este día

journal.window.facets.activities.evidence.duration.minutes =
    { $quantity ->
        [one] { $count } min
        [many] { $count } min
       *[other] { $count } min
    }

journal.window.facets.activities.evidence.segments.mixed =
    { $quantity ->
        [one] 1 segmento de más de una fuente, { $time_range }
        [many] { $count } segmentos de más de una fuente, { $time_range }
       *[other] { $count } segmentos de más de una fuente, { $time_range }
    }

journal.window.facets.activities.evidence.segments.named =
    { $quantity ->
        [one] 1 segmento de { $stream_name }, { $time_range }
        [many] { $count } segmentos de { $stream_name }, { $time_range }
       *[other] { $count } segmentos de { $stream_name }, { $time_range }
    }

journal.window.facets.activities.evidence.segments.unknown =
    { $quantity ->
        [one] 1 segmento de una fuente desconocida, { $time_range }
        [many] { $count } segmentos de una fuente desconocida, { $time_range }
       *[other] { $count } segmentos de una fuente desconocida, { $time_range }
    }

journal.window.facets.activities.evidence.show.all.count =
    { $quantity ->
        [one] mostrar { $count } segmento
        [many] mostrar todos los { $count } segmentos
       *[other] mostrar todos los { $count } segmentos
    }

journal.window.facets.activities.hidden.footer.count =
    { $quantity ->
        [one] { $count } oculta en este día
        [many] { $count } ocultas en este día
       *[other] { $count } ocultas en este día
    }

journal.window.facets.activities.hidden.showing.count =
    { $quantity ->
        [one] mostrando { $count } oculta
        [many] mostrando { $count } ocultas
       *[other] mostrando { $count } ocultas
    }

journal.window.facets.activities.history.changed.fields =
    { $field_group ->
       *[title] cambiaste el título
        [description] cambiaste la descripción
        [details] cambiaste tu relato
        [title-description] cambiaste el título y la descripción
        [title-details] cambiaste el título y tu relato
        [description-details] cambiaste la descripción y tu relato
        [title-description-details] cambiaste el título, la descripción y tu relato
    }

journal.window.facets.activities.history.changed.unclassified = cambiaste{ $slot1 }

journal.window.facets.activities.history.corrected.lines =
    { $quantity ->
        [one] corregiste quién dijo 1 línea
        [many] corregiste quién dijo { $count } líneas
       *[other] corregiste quién dijo { $count } líneas
    }

journal.window.facets.activities.history.found.stream = se encontró en segmentos de { $stream_name }

journal.window.facets.activities.history.remove.description = quitar: { $recorded_change }

journal.window.facets.activities.history.suggested.agent = sugerida por { $agent }

journal.window.facets.activities.line.correction.help = tu respuesta es para las líneas que marques, no para sus voces. para enseñarle una voz a tu diario, o poner en orden las voces que conoce, ve a { $speakers_start }hablantes{ $speakers_end }.

journal.window.facets.activities.line.drawer.count =
    { $quantity ->
        [one] 1 línea
        [many] { $count } líneas
       *[other] { $count } líneas
    }

journal.window.facets.activities.line.play.time = reproducir la línea a las { $time }

journal.window.facets.activities.line.question.selected =
    { $quantity ->
        [one] ¿quién dijo la línea que marcaste?
        [many] ¿quién dijo las { $count } líneas que marcaste?
       *[other] ¿quién dijo las { $count } líneas que marcaste?
    }

journal.window.facets.activities.line.refusal.owner.line.answers.unreadable = rechazo: línea ilegible

journal.window.facets.activities.line.refusal.owner.line.empty = rechazo: línea vacía

journal.window.facets.activities.line.refusal.owner.line.not.found = rechazo: línea cambiada

journal.window.facets.activities.line.refusal.owner.line.outside.activity = rechazo: línea fuera

journal.window.facets.activities.line.refusal.owner.line.owner.not.admitted = rechazo: línea de propietario

journal.window.facets.activities.line.refusal.owner.line.person.unavailable = rechazo: línea de persona

journal.window.facets.activities.line.refusal.owner.line.source.unsupported = rechazo: línea de fuente

journal.window.facets.activities.months.short.apr = abr

journal.window.facets.activities.months.short.aug = ago

journal.window.facets.activities.months.short.dec = dic

journal.window.facets.activities.months.short.feb = feb

journal.window.facets.activities.months.short.jan = ene

journal.window.facets.activities.months.short.jul = jul

journal.window.facets.activities.months.short.jun = jun

journal.window.facets.activities.months.short.mar = mar

journal.window.facets.activities.months.short.may = may

journal.window.facets.activities.months.short.nov = nov

journal.window.facets.activities.months.short.oct = oct

journal.window.facets.activities.months.short.sep = sep

journal.window.facets.activities.people.recorded.more =
    { $quantity ->
        [one] con { $first_person }, { $second_person } y 1 persona más
        [many] con { $first_person }, { $second_person } y { $others } personas más
       *[other] con { $first_person }, { $second_person } y { $others } personas más
    }

journal.window.facets.activities.people.recorded.one = con { $person }

journal.window.facets.activities.people.recorded.two = con { $first_person } y { $second_person }

journal.window.facets.activities.place.recorded.then = { $device_name } estaba en { $place } en ese momento

journal.window.facets.activities.response.detail.disallowed.fields = patch contiene campos no permitidos: { $field_list }

journal.window.facets.activities.response.detail.previous.answer.unreadable = no se puede leer la respuesta que esta edición reemplazó: { $read_error }

journal.window.facets.activities.response.detail.segment.outside.activity = { $segment_path } no es un segmento de esta actividad

journal.window.facets.activities.response.error.detail.activities.couldn.t.be.updated.right = no se pudieron actualizar las actividades ahora porque estaban ocupadas. vuelve a intentarlo en un momento.

journal.window.facets.activities.response.error.detail.day.must.not.be.after.today = el día no debe ser posterior a hoy

journal.window.facets.activities.response.error.detail.day.not.found = No se encontró el día

journal.window.facets.activities.response.error.detail.each.line.names.its.stream.segment = cada línea nombra su flujo, su segmento, su fuente y su sentence_id

journal.window.facets.activities.response.error.detail.facet.must.not.be.empty = la faceta no debe estar vacía

journal.window.facets.activities.response.error.detail.invalid.month.format.expected.yyyymm = Formato de mes no válido, se esperaba { $slot1 }

journal.window.facets.activities.response.error.detail.lines.must.be.a.list.of = las líneas deben ser una lista de líneas

journal.window.facets.activities.response.error.detail.only.a.record.in.a.group = solo un registro en un grupo de dos o más se puede separar

journal.window.facets.activities.response.error.detail.patch.must.be.a.non.empty = patch debe ser un objeto no vacío

journal.window.facets.activities.response.error.detail.patch.must.not.be.empty = patch no debe estar vacío

journal.window.facets.activities.response.error.detail.playback.must.be.true.or.false = playback debe ser true o false

journal.window.facets.activities.response.error.detail.position.timestamp.and.actor.name.the = posición, marca de tiempo y actor nombran la edición a quitar

journal.window.facets.activities.response.error.detail.removing.an.edit.needs.the.owner = quitar una edición requiere al propietario

journal.window.facets.activities.response.error.detail.saying.who.said.a.line.needs = decir quién dijo una línea requiere al propietario

journal.window.facets.activities.response.error.detail.separating.a.group.needs.the.owner = separar un grupo requiere al propietario

journal.window.facets.activities.response.error.detail.source.must.be.user = la fuente debe ser { $slot1 }

journal.window.facets.activities.response.error.detail.speaker.is.kind.owner.kind.person = el hablante es { $slot1 }, { $slot2 } o { $slot3 }

journal.window.facets.activities.response.error.detail.title.must.not.be.empty = el título no debe estar vacío

journal.window.facets.activities.response.error.message.activities.couldn.t.be.updated.right = no se pudieron actualizar las actividades ahora porque estaban ocupadas. vuelve a intentarlo en un momento.

journal.window.facets.activities.response.error.message.only.you.can.remove.a.correction = solo tú puedes quitar una corrección.

journal.window.facets.activities.response.error.message.only.you.can.say.who.said = solo tú puedes decir quién dijo estas líneas.

journal.window.facets.activities.response.error.message.only.you.can.separate.a.group = solo tú puedes separar un grupo.

journal.window.facets.activities.response.error.message.that.activity.couldn.t.be.created = no se pudo crear esa actividad porque ya existe.

journal.window.facets.activities.response.error.message.that.activity.isn.t.in.the = esa actividad no está en la faceta.

journal.window.facets.activities.response.error.message.that.activity.setting.couldn.t.be = no se pudo usar esa configuración de actividad.

journal.window.facets.activities.response.error.message.that.day.couldn.t.be.used = no se pudo usar ese día.

journal.window.facets.activities.response.error.message.that.edit.couldn.t.be.removed = no se pudo quitar esa edición.

journal.window.facets.activities.response.error.message.that.month.couldn.t.be.used = no se pudo usar ese mes.

journal.window.facets.activities.response.error.message.that.request.didn.t.finish = esa solicitud no terminó.

journal.window.facets.activities.response.error.message.this.isn.t.grouped.with.anything = esto no está agrupado con nada.

journal.window.facets.activities.response.error.message.this.record.is.being.moved.you = este registro se está moviendo. puedes editarlo cuando eso termine.

journal.window.facets.activities.response.error.message.who.said.those.lines.couldn.t = no se pudo guardar quién dijo esas líneas.

journal.window.facets.activities.scope.selected.facet = mostrando { $facet_name }

journal.window.facets.activities.timeline.item.hidden = { $time_range }, { $facet_name }, oculta: { $activity_title }

journal.window.facets.activities.timeline.item.hidden.source = { $time_range }, { $facet_name }, { $stream_name }, oculta: { $activity_title }

journal.window.facets.activities.timeline.item.visible = { $time_range }, { $facet_name }: { $activity_title }

journal.window.facets.activities.timeline.item.visible.source = { $time_range }, { $facet_name }, { $stream_name }: { $activity_title }

journal.window.facets.activities.window.explanation.a.line.shows.the.wrong.person = una línea muestra a la persona equivocada

journal.window.facets.activities.window.explanation.activities.appear.here.a.little.while = las actividades aparecen aquí un poco después de terminar, y todavía pueden crecer a medida que entra más.

journal.window.facets.activities.window.explanation.added.with.the.solstone.command = agregada con el comando de solstone

journal.window.facets.activities.window.explanation.as.the.day.goes.on.activities = a medida que pasa el día, aquí aparecen actividades de lo que compartes.

journal.window.facets.activities.window.explanation.can.t.be.read.right.now = no se puede leer en este momento

journal.window.facets.activities.window.explanation.changed.with.the.solstone.command = cambiada con el comando de solstone

journal.window.facets.activities.window.explanation.couldn.t.load.the.people.in = no se pudo cargar las personas de tu diario.

journal.window.facets.activities.window.explanation.couldn.t.remove.that.try.again = no se pudo quitar eso. vuelve a intentarlo en un momento.

journal.window.facets.activities.window.explanation.couldn.t.save.that.try.again = no se pudo guardar eso. vuelve a intentarlo en un momento.

journal.window.facets.activities.window.explanation.couldn.t.save.your.changes.are = no se pudo guardar. tus cambios siguen aquí, así que puedes volver a intentarlo.

journal.window.facets.activities.window.explanation.found.in.segments.from.an.unknown = se encontró en segmentos de una fuente desconocida

journal.window.facets.activities.window.explanation.found.in.segments.from.more.than = se encontró en segmentos de más de una fuente

journal.window.facets.activities.window.explanation.grew.as.more.of.it.came = creció a medida que llegaba más

journal.window.facets.activities.window.explanation.hidden.it.s.still.in.your = oculta. sigue en tu diario.

journal.window.facets.activities.window.explanation.hidden.with.the.solstone.command = oculta con el comando de solstone

journal.window.facets.activities.window.explanation.it.won.t.join.this.group = no volverá a unirse a este grupo. no se elimina nada.

journal.window.facets.activities.window.explanation.lines.your.journal.credits.to.you = líneas que tu diario te atribuye

journal.window.facets.activities.window.explanation.more.than.one.source = más de una fuente

journal.window.facets.activities.window.explanation.no.activities.on.this.day = no hay actividades en este día

journal.window.facets.activities.window.explanation.no.longer.in.your.journal = ya no está en tu diario

journal.window.facets.activities.window.explanation.no.one.in.your.journal.matches = nadie en tu diario coincide con eso.

journal.window.facets.activities.window.explanation.on.your.calendar.then = en tu calendario en ese momento

journal.window.facets.activities.window.explanation.other.facets.have.activities.on.this = otras facetas tienen actividades en este día.

journal.window.facets.activities.window.explanation.part.of.it.removed = se quitó una parte

journal.window.facets.activities.window.explanation.pick.an.activity.to.see.what = elige una actividad para ver en qué se basa.

journal.window.facets.activities.window.explanation.say.it.the.way.you.d = dilo como se lo contarías a alguien cercano: quién hizo qué, qué se decidió, qué sigue abierto.

journal.window.facets.activities.window.explanation.seen.as.cancelled.or.declined = vista como cancelada o rechazada

journal.window.facets.activities.window.explanation.some.of.those.lines.aren.t = algunas de esas líneas no son parte de esta actividad.

journal.window.facets.activities.window.explanation.someone.i.don.t.know = alguien que no conozco

journal.window.facets.activities.window.explanation.tell.your.journal.in.your.own = cuéntale a tu diario con tus propias palabras

journal.window.facets.activities.window.explanation.that.changed.since.it.was.shown = eso cambió desde que se mostró. así está ahora.

journal.window.facets.activities.window.explanation.that.person.can.t.be.chosen = no se puede elegir a esa persona. elige a alguien más, o a alguien que no conoces.

journal.window.facets.activities.window.explanation.the.journal.adds.activities.as.it = el diario agrega actividades a medida que revisa lo que has compartido.

journal.window.facets.activities.window.explanation.these.lines.can.t.take.an = estas líneas no aceptan una respuesta sobre quién las dijo. en su lugar, puedes contarle a tu diario qué pasó.

journal.window.facets.activities.window.explanation.these.lines.have.changed.since.they = estas líneas han cambiado desde que se mostraron. vuelve atrás y ábrelas de nuevo.

journal.window.facets.activities.window.explanation.this.was.a.recording.or.video = esto era una grabación o un video en reproducción

journal.window.facets.activities.window.explanation.tick.the.lines.you.want.to = marca primero las líneas que quieres corregir.

journal.window.facets.activities.window.explanation.unhidden.with.the.solstone.command = se dejó de ocultar con el comando de solstone

journal.window.facets.activities.window.explanation.unhide.the.day.on.the.body = deja de ocultar el día en la página del cuerpo

journal.window.facets.activities.window.explanation.what.it.s.based.on = en qué se basa

journal.window.facets.activities.window.explanation.who.said.the.lines.you.ticked = ¿quién dijo las líneas que marcaste?

journal.window.facets.activities.window.explanation.yes.it.was.playing = sí, estaba en reproducción

journal.window.facets.activities.window.explanation.you.hid.this.day.s.body = ocultaste los datos del cuerpo de este día

journal.window.facets.activities.window.explanation.you.said.this.was.a.recording = dijiste que esto era una grabación o un video en reproducción

journal.window.facets.activities.window.explanation.you.told.your.journal.what.happened = le contaste a tu diario qué pasó

journal.window.facets.activities.window.explanation.your.account.of.it = tu relato de esto

journal.window.facets.activities.window.explanation.your.earlier.answers.about.who.spoke = no se pueden leer tus respuestas anteriores sobre quién habló aquí, así que esta no se guardó.

journal.window.facets.activities.window.explanation.your.journal.doesn.t.know.which = tu diario todavía no sabe qué persona eres, así que no puede marcar las líneas como tuyas.

journal.window.facets.activities.window.explanation.your.journal.will.think.this.through = tu diario va a reconsiderarlo como algo que viste o escuchaste, sin que nadie más participe.

journal.window.facets.awareness.response.detail.multiple.import.answers = proporciona exactamente uno de { $slot1 }; se recibió { $answer_list }

journal.window.facets.awareness.response.detail.unknown.section = no hay una sección de conciencia llamada '{ $section_name }'

journal.window.facets.awareness.response.detail.unreadable.settings = no se pudo leer tu archivo de configuración en { $settings_path }. no se cambió tu configuración. repara el archivo o restaura { $slot3 } desde una copia de seguridad y luego vuelve a intentarlo.

journal.window.facets.awareness.response.error.detail.kind.is.required = kind es obligatorio

journal.window.facets.awareness.response.error.detail.no.request.body = no hay cuerpo de la solicitud

journal.window.facets.awareness.response.error.detail.request.body.must.be.a.json = el cuerpo de la solicitud debe ser un objeto { $slot1 }

journal.window.facets.awareness.response.error.message.a.required.field.is.missing = falta un campo obligatorio.

journal.window.facets.awareness.response.error.message.one.of.those.values.couldn.t = no se pudo usar uno de esos valores.

journal.window.facets.awareness.response.error.message.that.json.request.couldn.t.be = no se pudo leer esa solicitud { $slot1 }.

journal.window.facets.awareness.response.error.message.that.part.of.your.journal.couldn = no se pudo encontrar esa parte de tu diario.

journal.window.facets.awareness.response.error.message.that.request.didn.t.finish = esa solicitud no terminó.

journal.window.facets.awareness.response.error.message.that.request.had.no.data.in = esa solicitud no tenía datos.

journal.window.facets.awareness.response.error.message.the.awareness.operation.is.busy = La operación de conciencia está ocupada.

journal.window.facets.awareness.response.error.message.your.settings.couldn.t.be.read = no se pudo leer tu configuración.

journal.window.facets.awareness.response.imports.busy.detail = las importaciones están ocupadas; vuelve a intentarlo

journal.window.facets.awareness.response.one.import.answer = proporciona exactamente uno de { $slot1 }

journal.window.facets.curation.ambiguity.choose.name = elegir { $candidate_name }

journal.window.facets.curation.ambiguity.member.occurrences =
    { $quantity ->
        [one] { $count } ocurrencia
        [many] { $count } ocurrencias
       *[other] { $count } ocurrencias
    }

journal.window.facets.curation.ambiguity.occurrences =
    { $quantity ->
        [one] { $count } ocurrencia
        [many] { $count } ocurrencias
       *[other] { $count } ocurrencias
    }

journal.window.facets.curation.ambiguity.question = ¿qué entrada coincide con "{ $ambiguous_name }"?

journal.window.facets.curation.button.label.try.again = reintentar

journal.window.facets.curation.disclosure.label.individual.questions.and.sources = preguntas y fuentes individuales

journal.window.facets.curation.disclosure.label.source.evidence = evidencia de la fuente

journal.window.facets.curation.disclosure.label.source.identifiers = identificadores de la fuente

journal.window.facets.curation.disclosure.label.technical.details = detalles técnicos

journal.window.facets.curation.evidence.drawer.title = evidencia

journal.window.facets.curation.evidence.showing.partial =
    { $shown_quantity ->
        [one] mostrando { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
        [many] mostrando { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
       *[other] mostrando { $shown } de { $total_quantity ->
            [one] { $total }
            [many] { $total }
           *[other] { $total }
        }
    }

journal.window.facets.curation.evidence.total.pieces =
    { $quantity ->
        [one] 1 pieza
        [many] { $count } piezas
       *[other] { $count } piezas
    }

journal.window.facets.curation.facet.additional.names = también aparece como { $alias_list }.

journal.window.facets.curation.facet.create.question = la actividad del diario no encaja bien con tus facetas. ¿crear la faceta "{ $facet_name }"?

journal.window.facets.curation.list.items = { $preceding_items } y { $last_item }

journal.window.facets.curation.placeholder.this.is.a.speaker.label.not = esta es una etiqueta de hablante, no un nombre

journal.window.facets.curation.preview.activities.count =
    { $quantity ->
        [one] registros de actividad cambiados: { $count }
        [many] registros de actividad cambiados: { $count }
       *[other] registros de actividad cambiados: { $count }
    }

journal.window.facets.curation.preview.aliases = alias agregados: { $aliases }

journal.window.facets.curation.preview.emails.count =
    { $quantity ->
        [one] correos agregados: { $count }
        [many] correos agregados: { $count }
       *[other] correos agregados: { $count }
    }

journal.window.facets.curation.preview.facet.links =
    { $moved_quantity ->
        [one] enlaces de faceta: { $moved } movido, { $merged_quantity ->
            [one] { $merged } fusionado
            [many] { $merged } fusionados
           *[other] { $merged } fusionados
        }
        [many] enlaces de faceta: { $moved } movidos, { $merged_quantity ->
            [one] { $merged } fusionado
            [many] { $merged } fusionados
           *[other] { $merged } fusionados
        }
       *[other] enlaces de faceta: { $moved } movidos, { $merged_quantity ->
            [one] { $merged } fusionado
            [many] { $merged } fusionados
           *[other] { $merged } fusionados
        }
    }

journal.window.facets.curation.preview.notes.count =
    { $quantity ->
        [one] notas bajo el nombre que se fusiona: { $count }
        [many] notas bajo el nombre que se fusiona: { $count }
       *[other] notas bajo el nombre que se fusiona: { $count }
    }

journal.window.facets.curation.preview.observations.count =
    { $quantity ->
        [one] notas movidas: { $count }
        [many] notas movidas: { $count }
       *[other] notas movidas: { $count }
    }

journal.window.facets.curation.preview.speaker.labels.count =
    { $quantity ->
        [one] etiquetas de hablante cambiadas: { $count }
        [many] etiquetas de hablante cambiadas: { $count }
       *[other] etiquetas de hablante cambiadas: { $count }
    }

journal.window.facets.curation.preview.speaker.segments =
    { $labels_quantity ->
        [one] etiquetas de hablante actualizadas: { $labels } etiqueta, { $corrections_quantity ->
            [one] { $corrections } corrección
            [many] { $corrections } correcciones
           *[other] { $corrections } correcciones
        }
        [many] etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections_quantity ->
            [one] { $corrections } corrección
            [many] { $corrections } correcciones
           *[other] { $corrections } correcciones
        }
       *[other] etiquetas de hablante actualizadas: { $labels } etiquetas, { $corrections_quantity ->
            [one] { $corrections } corrección
            [many] { $corrections } correcciones
           *[other] { $corrections } correcciones
        }
    }

journal.window.facets.curation.preview.voice.samples =
    { $added_quantity ->
        [one] muestras de voz movidas: { $added } agregada, { $total_quantity ->
            [one] { $total } en total
            [many] { $total } en total
           *[other] { $total } en total
        }
        [many] muestras de voz movidas: { $added } agregadas, { $total_quantity ->
            [one] { $total } en total
            [many] { $total } en total
           *[other] { $total } en total
        }
       *[other] muestras de voz movidas: { $added } agregadas, { $total_quantity ->
            [one] { $total } en total
            [many] { $total } en total
           *[other] { $total } en total
        }
    }

journal.window.facets.curation.response.batch.candidate.identity.missing = a la candidata le falta facet, source_slug o target_slug

journal.window.facets.curation.response.candidate.absent = no se encontró la candidata

journal.window.facets.curation.response.candidate.accept.status = no se puede aceptar la candidata con estado{ $slot1 }{ $status }

journal.window.facets.curation.response.candidate.dismiss.status = no se puede descartar la candidata con estado{ $slot1 }{ $status }

journal.window.facets.curation.response.candidate.kept.name.missing = "{ $name }" ya está fusionado en "{ $kept }", sin conservar su nombre. agrega "{ $name }" a los nombres alternativos de "{ $kept }" en la página de entidades y luego acepta de nuevo.

journal.window.facets.curation.response.candidate.preview.status = no se puede previsualizar la candidata con estado{ $slot1 }{ $status }

journal.window.facets.curation.response.copy.action.failed = eso no se completó. vuelve a intentarlo.

journal.window.facets.curation.response.copy.ambiguity.body = ¿qué entrada coincide con "{ $slot1 }"?

journal.window.facets.curation.response.copy.ambiguity.choose.action = elegir { $slot1 }

journal.window.facets.curation.response.copy.ambiguity.dismiss.action = ninguna de estas

journal.window.facets.curation.response.copy.ambiguity.origin.label = se notó en

journal.window.facets.curation.response.copy.empty.state = nada para revisar. todavía no hay una estructura nueva para sugerir.

journal.window.facets.curation.response.copy.entity.batch.confirm.dismiss.action = dejar todos por separado

journal.window.facets.curation.response.copy.entity.batch.confirm.merge.action = fusionar todo

journal.window.facets.curation.response.copy.entity.batch.dismiss.action = dejar la selección por separado

journal.window.facets.curation.response.copy.entity.batch.dismiss.lead = estos pares se van a dejar por separado:

journal.window.facets.curation.response.copy.entity.batch.dismiss.summary = se dejaron { $slot1 } de { $slot2 } por separado.

journal.window.facets.curation.response.copy.entity.batch.failed.note = { $slot1 } todavía necesitan atención abajo.

journal.window.facets.curation.response.copy.entity.batch.merge.action = fusionar la selección

journal.window.facets.curation.response.copy.entity.batch.merge.lead = estos pares se van a fusionar. las fusiones no se pueden deshacer:

journal.window.facets.curation.response.copy.entity.batch.merge.summary = se fusionaron { $slot1 } de { $slot2 }.

journal.window.facets.curation.response.copy.entity.body = "{ $slot1 }" y "{ $slot2 }" parecen la misma entidad. ¿las fusionas?

journal.window.facets.curation.response.copy.entity.cancel.action = cancelar

journal.window.facets.curation.response.copy.entity.confirm.action = confirmar fusión

journal.window.facets.curation.response.copy.entity.dismiss.action = dejar por separado

journal.window.facets.curation.response.copy.entity.done.for.now.action = listo por ahora

journal.window.facets.curation.response.copy.entity.facets.label = (en { $slot1 })

journal.window.facets.curation.response.copy.entity.group.lede = estos { $slot1 } pares se parecen. fusionarlos evita que se mezclen tus nombres.

journal.window.facets.curation.response.copy.entity.merge.action = fusionar

journal.window.facets.curation.response.copy.entity.preview.empty = esta fusión no necesita cambios en el diario.

journal.window.facets.curation.response.copy.entity.preview.errors = algunas actualizaciones de segmentos pueden necesitar atención.

journal.window.facets.curation.response.copy.entity.preview.lead = antes de fusionar, esto es lo que va a cambiar. una fusión no se puede deshacer.

journal.window.facets.curation.response.copy.entity.progress.label = se revisaron { $slot1 } · quedan { $slot2 }

journal.window.facets.curation.response.copy.entity.select.all.action = seleccionar todo

journal.window.facets.curation.response.copy.entity.show.rest.action = mostrar el resto ({ $slot1 })

journal.window.facets.curation.response.copy.facet.body = la actividad del diario no encaja bien con tus facetas. ¿creas la faceta "{ $slot1 }"?

journal.window.facets.curation.response.copy.facet.create.action = crear faceta

journal.window.facets.curation.response.copy.facet.dismiss.action = ahora no

journal.window.facets.curation.response.copy.heading = sugerencias

journal.window.facets.curation.response.copy.preview.activities = registros de actividad cambiados

journal.window.facets.curation.response.copy.preview.akas.label = alias agregados

journal.window.facets.curation.response.copy.preview.emails.label = correos agregados

journal.window.facets.curation.response.copy.preview.facets.label = enlaces de facetas

journal.window.facets.curation.response.copy.preview.labels = etiquetas de hablante cambiadas

journal.window.facets.curation.response.copy.preview.notes.source = notas del nombre que se fusiona

journal.window.facets.curation.response.copy.preview.observations.label = notas movidas

journal.window.facets.curation.response.copy.preview.partial = la fusión también une enlaces de facetas y fotos.

journal.window.facets.curation.response.copy.preview.segments.label = etiquetas de hablante actualizadas

journal.window.facets.curation.response.copy.preview.voiceprints.label = muestras de voz movidas

journal.window.facets.curation.response.copy.repair.required = { $slot1 } { $slot2 }

journal.window.facets.curation.response.copy.review.restore.action = devolver a revisión

journal.window.facets.curation.response.copy.speaker.body = "{ $slot1 }" y "{ $slot2 }" pueden ser el mismo hablante. ¿los fusionas?

journal.window.facets.curation.response.copy.speaker.candidate.pair.body = dos voces en tu diario suenan parecido. ¿las fusionas?

journal.window.facets.curation.response.copy.speaker.candidate.pair.dismiss.action = dejar por separado

journal.window.facets.curation.response.copy.speaker.candidate.pair.intervals.label = intervalos

journal.window.facets.curation.response.copy.speaker.candidate.pair.merge.action = fusionar candidatas

journal.window.facets.curation.response.copy.speaker.candidate.pair.similarity.label = coseno

journal.window.facets.curation.response.copy.speaker.candidate.pair.source.label = candidata A

journal.window.facets.curation.response.copy.speaker.candidate.pair.target.label = candidata B

journal.window.facets.curation.response.copy.speaker.dismiss.action = dejar por separado

journal.window.facets.curation.response.copy.speaker.merge.action = revisar fusión

journal.window.facets.curation.response.detail.missing.field = Falta { $field_name }

journal.window.facets.curation.response.detail.unknown.group = grupo de curación desconocido: { $group }

journal.window.facets.curation.response.detail.unreadable.settings = no se pudo leer tu archivo de configuración en { $settings_path }. no se cambió tu configuración. repara el archivo o restaura { $slot3 } desde una copia de seguridad y luego vuelve a intentarlo.

journal.window.facets.curation.response.entity.busy.detail = las candidatas de fusión de entidades están ocupadas; vuelve a intentarlo

journal.window.facets.curation.response.error.detail.key.does.not.match.anchor.a = la clave no coincide con { $slot1 }

journal.window.facets.curation.response.error.detail.key.does.not.match.source.id = la clave no coincide con { $slot1 }

journal.window.facets.curation.response.error.message.a.required.field.is.missing = falta un campo obligatorio.

journal.window.facets.curation.response.error.message.one.of.those.values.couldn.t = no se pudo usar uno de esos valores.

journal.window.facets.curation.response.error.message.that.entity.operation.couldn.t.be = no se pudo completar esa operación de entidad.

journal.window.facets.curation.response.error.message.that.list.isn.t.one.curation = esa lista no se recorre en las páginas de curación.

journal.window.facets.curation.response.error.message.the.entity.operation.is.busy = La operación de entidad está ocupada.

journal.window.facets.curation.response.error.message.your.settings.couldn.t.be.read = no se pudo leer tu configuración.

journal.window.facets.curation.response.pair.already.confirmed = no se pueden fusionar dos candidatas confirmadas

journal.window.facets.curation.response.pair.already.merged = el par de candidatas ya está fusionado

journal.window.facets.curation.response.pair.anchor.absent = no se encontró el ancla candidata

journal.window.facets.curation.response.pair.below.threshold = el par de candidatas está bajo el umbral de revisión

journal.window.facets.curation.response.pair.rejected = no se puede fusionar una candidata rechazada

journal.window.facets.curation.response.speaker.busy.detail = las sugerencias de hablantes están ocupadas; vuelve a intentarlo

journal.window.facets.curation.review.total =
    { $quantity ->
        [one] { $count } sugerencia para revisar. elige un grupo para empezar.
        [many] { $count } sugerencias para revisar. elige un grupo para empezar.
       *[other] { $count } sugerencias para revisar. elige un grupo para empezar.
    }

journal.window.facets.curation.speaker.merge.question = "{ $source_name }" y "{ $target_name }" pueden ser el mismo hablante. ¿los fusionas?

journal.window.facets.curation.status.text.journal = diario

journal.window.facets.curation.status.text.please.select.at.least.one.question = por favor, selecciona al menos una pregunta para resolver.

journal.window.facets.curation.status.text.restore.to.review = devolver a revisión

journal.window.facets.curation.suggestion.display.text.another.change.is.in.progress.try = hay otro cambio en curso. vuelve a intentarlo en un momento.

journal.window.facets.curation.suggestion.display.text.automatic.review = revisión automática

journal.window.facets.curation.suggestion.display.text.couldn.t.apply.that.choice.refresh = no se pudo aplicar esa elección. actualiza las sugerencias y vuelve a intentarlo.

journal.window.facets.curation.suggestion.display.text.couldn.t.save.that.change.try = no se pudo guardar ese cambio. vuelve a intentarlo.

journal.window.facets.curation.suggestion.display.text.facet.suggestions = sugerencias de facetas

journal.window.facets.curation.suggestion.display.text.names.to.clarify = nombres por aclarar

journal.window.facets.curation.suggestion.display.text.possible.duplicate.voices = posibles voces duplicadas

journal.window.facets.curation.suggestion.display.text.show.more = mostrar más

journal.window.facets.curation.suggestion.display.text.this.suggestion.or.one.of.its = esta sugerencia o una de sus coincidencias ya no está disponible. actualiza las sugerencias.

journal.window.facets.curation.suggestions.accept = aceptar

journal.window.facets.curation.suggestions.account.attributed.agent = tu relato, como lo escribió { $agent_name }

journal.window.facets.curation.suggestions.basis.agent.journal.shows = { $agent_name } dice que notó esto en tu diario

journal.window.facets.curation.suggestions.basis.agent.my.own.work = { $agent_name } dice que lo sabe de primera mano

journal.window.facets.curation.suggestions.basis.agent.outside.source = { $agent_name } dice que encontró esto fuera de tu diario

journal.window.facets.curation.suggestions.basis.agent.owner.said = { $agent_name } dice que se lo dijiste

journal.window.facets.curation.suggestions.cancel = cancelar

journal.window.facets.curation.suggestions.dismiss = descartar

journal.window.facets.curation.suggestions.earlier.accepted = lo aceptaste

journal.window.facets.curation.suggestions.earlier.agent = un agente anterior

journal.window.facets.curation.suggestions.earlier.dismissed = lo descartaste

journal.window.facets.curation.suggestions.earlier.in.duplicates = en posibles duplicados

journal.window.facets.curation.suggestions.earlier.not.accepted = sin aceptar

journal.window.facets.curation.suggestions.earlier.stale = ya cambió

journal.window.facets.curation.suggestions.edit = editar

journal.window.facets.curation.suggestions.edit.name.label = nombre

journal.window.facets.curation.suggestions.filter.empty.agent = no hay nada de { $agent_name } en espera.

journal.window.facets.curation.suggestions.filter.only.agent = solo de { $agent_name }.

journal.window.facets.curation.suggestions.group = de tus agentes

journal.window.facets.curation.suggestions.keep.name = nombre a usar

journal.window.facets.curation.suggestions.kind.name = un nombre

journal.window.facets.curation.suggestions.kind.same.person = la misma persona

journal.window.facets.curation.suggestions.kind.what.happened = qué pasó en realidad

journal.window.facets.curation.suggestions.kind.who.said.it = quién lo dijo

journal.window.facets.curation.suggestions.lines.assigned.named =
    { $quantity ->
        [one] esta línea fue dicha por { $speaker }
        [many] estas { $count } líneas fueron dichas por { $speaker }
       *[other] estas { $count } líneas fueron dichas por { $speaker }
    }

journal.window.facets.curation.suggestions.lines.assigned.owner =
    { $quantity ->
        [one] esta línea fue dicha por { $speaker_start }ti{ $speaker_end }
        [many] estas { $count } líneas fueron dichas por { $speaker_start }ti{ $speaker_end }
       *[other] estas { $count } líneas fueron dichas por { $speaker_start }ti{ $speaker_end }
    }

journal.window.facets.curation.suggestions.lines.assigned.unknown =
    { $quantity ->
        [one] esta línea fue dicha por { $speaker_start }alguien que no conoces{ $speaker_end }
        [many] estas { $count } líneas fueron dichas por { $speaker_start }alguien que no conoces{ $speaker_end }
       *[other] estas { $count } líneas fueron dichas por { $speaker_start }alguien que no conoces{ $speaker_end }
    }

journal.window.facets.curation.suggestions.name.alias = { $former_name }, también como { $new_name }

journal.window.facets.curation.suggestions.name.change = { $former_name } → { $new_name }

journal.window.facets.curation.suggestions.now = tu diario dice

journal.window.facets.curation.suggestions.now.people = tu diario tiene

journal.window.facets.curation.suggestions.ok = aceptar

journal.window.facets.curation.suggestions.open = abrir

journal.window.facets.curation.suggestions.open.activity = abrir la actividad

journal.window.facets.curation.suggestions.open.duplicates = abrir posibles duplicados

journal.window.facets.curation.suggestions.open.lines = abrir las líneas

journal.window.facets.curation.suggestions.open.person.name = abrir { $person_name }

journal.window.facets.curation.suggestions.person.facet.membership = en { $facet_list }

journal.window.facets.curation.suggestions.person.notes.count =
    { $quantity ->
        [one] 1 nota
        [many] { $count } notas
       *[other] { $count } notas
    }

journal.window.facets.curation.suggestions.receipt.accepted.alias = se aceptó la sugerencia de { $agent_name }. { $former_name } ahora también se llama { $new_name }.

journal.window.facets.curation.suggestions.receipt.accepted.ask = se aceptó la sugerencia de { $agent_name }. tu diario va a volver a pensar en esto.

journal.window.facets.curation.suggestions.receipt.accepted.ask.unconfirmed = se aceptó la sugerencia de { $agent_name } y se guardó, pero tu diario no pudo confirmar que va a volver a pensar en esto.

journal.window.facets.curation.suggestions.receipt.accepted.merged.people = se aceptó la sugerencia de { $agent_name }. { $merged_name } y { $kept_name } ahora son una sola persona, llamada { $kept_name }.

journal.window.facets.curation.suggestions.receipt.accepted.partial = se aceptó la sugerencia de { $agent_name } para esta actividad. no se pudieron actualizar algunas otras actividades.

journal.window.facets.curation.suggestions.receipt.accepted.rename = se aceptó la sugerencia de { $agent_name }. { $former_name } ahora es { $new_name }.

journal.window.facets.curation.suggestions.receipt.accepted.simple = se aceptó la sugerencia de { $agent_name }.

journal.window.facets.curation.suggestions.receipt.dismissed.agent = se descartó. si { $agent_name } vuelve a sugerir esto, a { $agent_name } se le dice que ya dijiste que no.

journal.window.facets.curation.suggestions.receipt.undone.alias = deshecho. { $new_name } ya no es otro nombre de { $former_name }.

journal.window.facets.curation.suggestions.receipt.undone.rename = deshecho. { $new_name } es { $former_name } otra vez.

journal.window.facets.curation.suggestions.retry = reintentar

journal.window.facets.curation.suggestions.show.all = mostrar todos los agentes

journal.window.facets.curation.suggestions.show.more = mostrar más

journal.window.facets.curation.suggestions.stale.changed.after.agent = esto cambió después de que { $agent_name } lo sugirió, así que no hay nada que aceptar. así está ahora:

journal.window.facets.curation.suggestions.stale.chip = ya cambió

journal.window.facets.curation.suggestions.stale.pair.changed.after.agent = { $first_name } o { $second_name } cambió después de que { $agent_name } sugirió esto, así que no hay nada que aceptar.

journal.window.facets.curation.suggestions.stale.pair.journal.candidate = tu diario ya está preguntando si { $first_name } y { $second_name } son la misma persona. puedes responder eso en posibles duplicados.

journal.window.facets.curation.suggestions.stale.pair.journal.question = tu diario tiene su propia pregunta sobre { $first_name } y { $second_name }, así que aquí no hay nada que aceptar.

journal.window.facets.curation.suggestions.stale.pair.owner.answered = ya dejaste a { $first_name } y a { $second_name } por separado, así que no hay nada que aceptar.

journal.window.facets.curation.suggestions.suggested = sugerido

journal.window.facets.curation.suggestions.then = si aceptas

journal.window.facets.curation.suggestions.then.account.days = tu diario vuelve a pensar con tu relato en esta actividad, y en { $affected_days }.

journal.window.facets.curation.suggestions.then.activities.days =
    { $quantity ->
        [one] tu diario vuelve a pensar en esta actividad, y en { $affected_days }.
        [many] tu diario vuelve a pensar en estas { $count } actividades, y en { $affected_days }.
       *[other] tu diario vuelve a pensar en estas { $count } actividades, y en { $affected_days }.
    }

journal.window.facets.curation.suggestions.then.alias.name = tu diario también llama a esta persona { $new_name }. la versión anterior se queda en el historial de versiones, donde puedes restaurarla.

journal.window.facets.curation.suggestions.then.merge.people = tu diario guarda una sola persona llamada { $kept_name }, también llamada { $merged_name }, y mueve todo lo asociado a { $merged_name } a esa persona. una fusión no se puede deshacer.

journal.window.facets.curation.suggestions.then.retain.previous.name = tu diario llama a esta persona { $new_name } de ahora en adelante. { $former_name } se queda en el historial de versiones, donde puedes restaurarlo.

journal.window.facets.curation.suggestions.undo = deshacer

journal.window.facets.curation.suggestions.undone.plain = deshecho.

journal.window.facets.curation.suggestions.updating = actualizando…

journal.window.facets.curation.suggestions.why = por qué

journal.window.facets.curation.text.loading.suggestions = cargando sugerencias…

journal.window.facets.curation.visible.text.cluster = grupo:

journal.window.facets.curation.visible.text.could.not.load.suggestions = no se pudieron cargar las sugerencias.

journal.window.facets.curation.visible.text.day = día:

journal.window.facets.curation.visible.text.loading.suggestions = cargando sugerencias…

journal.window.facets.curation.visible.text.segment = segmento:

journal.window.facets.curation.visible.text.source = fuente:

journal.window.facets.curation.visible.text.stream = flujo:

journal.window.facets.curation.window.explanation.a.close.spelling.match.was.chosen = se eligió automáticamente una coincidencia de ortografía cercana

journal.window.facets.curation.window.explanation.couldn.t.accept.that.so.nothing = no se pudo aceptar eso, así que nada cambió. vuelve a intentarlo en un momento.

journal.window.facets.curation.window.explanation.couldn.t.load.what.your.agents = no se pudo cargar lo que sugirieron tus agentes.

journal.window.facets.curation.window.explanation.couldn.t.undo.that.because.you = no se pudo deshacer eso, porque lo cambiaste desde entonces. tu cambio se queda.

journal.window.facets.curation.window.explanation.earlier.from.your.agents = lo anterior de tus agentes

journal.window.facets.curation.window.explanation.merge.them.into.one.person = fusionarlas en una sola persona

journal.window.facets.curation.window.explanation.possible.duplicate.speaker.names = posibles nombres de hablante duplicados

journal.window.facets.curation.window.explanation.saved.for.some.activities = se guardó para algunas actividades

journal.window.facets.curation.window.explanation.that.didn.t.go.through.try = eso no se completó. vuelve a intentarlo.

journal.window.facets.curation.window.explanation.the.names.have.little.in.common = los nombres tienen poco en común

journal.window.facets.curation.window.explanation.the.suggested.entity.is.no.longer = la entidad sugerida ya no está aquí

journal.window.facets.curation.window.explanation.the.suggested.entity.is.not.a = la entidad sugerida no es una persona

journal.window.facets.curation.window.explanation.this.can.t.be.accepted.right = esto no se puede aceptar ahora mismo.

journal.window.facets.curation.window.explanation.this.was.a.recording.or.video = esto era una grabación o un video en reproducción

journal.window.facets.curation.window.explanation.undone.but.your.journal.couldn.t = deshecho, pero tu diario no pudo confirmar que va a volver a pensar en esto.

journal.window.facets.curation.window.explanation.undone.your.journal.will.think.about = deshecho. tu diario va a volver a pensar en esto sin ese cambio.

journal.window.facets.curation.window.explanation.what.you.accept.is.kept.as = lo que aceptas se guarda como tu propio relato.

journal.window.facets.curation.window.explanation.you.accepted.it.then.undid.it = lo aceptaste y luego lo deshiciste

journal.window.facets.curation.window.explanation.you.kept.them.separate = los dejaste por separado

journal.window.facets.curation.window.explanation.your.account.of.it = tu relato de esto

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so = tu diario no respondió, así que esto puede que ya esté hecho. volver a intentarlo no lo hará dos veces.

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so.variant2 = tu diario no respondió, así que esto puede que ya esté aceptado. volver a intentarlo no lo aceptará dos veces.

journal.window.facets.curation.window.explanation.your.journal.will.think.this.through = tu diario va a reconsiderarlo como algo que viste o escuchaste, sin que nadie más participe.

journal.window.location.day.area.unnamed = sin una localidad con nombre cerca ({ $area })

journal.window.location.day.empty = se guardó la ubicación, pero no se pudo nombrar ninguna lectura.

journal.window.location.day.heading = ubicación el { $date }

journal.window.location.day.instant = a las { $time }

journal.window.location.day.moment = { $date } a las { $clock }

journal.window.location.day.nearby.list = cerca de { $leading } y de { $last }

journal.window.location.day.place.context = { $specific }; { $broader }

journal.window.location.day.place.inside = en { $place }, dentro de { $inside }

journal.window.location.day.place.named = en { $owner_name } (un lugar que nombraste); { $location }

journal.window.location.day.readings.area =
    { $quantity ->
        [one] lecturas { $area }, { $when }: { $count } lectura de { $device }.
        [many] lecturas { $area }, { $when }: { $count } lecturas de { $device }.
       *[other] lecturas { $area }, { $when }: { $count } lecturas de { $device }.
    }

journal.window.location.day.readings.area.approximate =
    { $quantity ->
        [one] lecturas { $area }, { $when }: { $count } lectura aproximada de { $device }.
        [many] lecturas { $area }, { $when }: { $count } lecturas aproximadas de { $device }.
       *[other] lecturas { $area }, { $when }: { $count } lecturas aproximadas de { $device }.
    }

journal.window.location.day.readings.unlocated =
    { $quantity ->
        [one] lecturas, { $when }: { $count } lectura de { $device }.
        [many] lecturas, { $when }: { $count } lecturas de { $device }.
       *[other] lecturas, { $when }: { $count } lecturas de { $device }.
    }

journal.window.location.day.readings.unlocated.approximate =
    { $quantity ->
        [one] lecturas, { $when }: { $count } lectura aproximada de { $device }.
        [many] lecturas, { $when }: { $count } lecturas aproximadas de { $device }.
       *[other] lecturas, { $when }: { $count } lecturas aproximadas de { $device }.
    }

journal.window.location.day.stay = una estancia que tu diario calculó a partir de { $count } lecturas de { $device } { $location }: primera lectura { $first }, última { $last }.

journal.window.location.day.visit = una visita que { $device } reportó { $location }: { $arrival }, { $departure }.

journal.window.map.accessible.name.edit.this.place = editar este lugar

journal.window.map.accessible.name.location.accuracy = precisión de ubicación

journal.window.map.accessible.name.map = mapa

journal.window.map.accessible.name.map.of.the.records.listed.here = mapa de los registros listados aquí, con localidades y fronteras

journal.window.map.accessible.name.more.of.your.location = más de tu ubicación

journal.window.map.accessible.name.other.weeks.and.this.month = otras semanas y este mes

journal.window.map.accessible.name.the.day.in.order = el día, en orden

journal.window.map.accessible.name.the.stops.across.the.day.shown = las paradas a lo largo del día, mostradas en la lista de abajo

journal.window.map.accessible.name.visits.and.stays.across.the.day = visitas y estancias a lo largo del día

journal.window.map.accessible.name.zoom = zoom

journal.window.map.accessible.name.zoom.in = acercar

journal.window.map.accessible.name.zoom.out = alejar

journal.window.map.accuracy.download.button = descargar { $size }

journal.window.map.accuracy.download.explanation = son{ $slot1 }{ $size }{ $slot2 }de { $slot3 }, la misma descarga para todo el mundo, así que obtenerla no dice nada sobre dónde has estado. los nombres se buscan en tu computadora.

journal.window.map.accuracy.download.progress = descargando precisión de ubicación:{ $slot1 }{ $received }{ $slot2 }de{ $slot3 }{ $total }. puedes seguir usando tu diario mientras se descarga.

journal.window.map.accuracy.explanation.names = una visita que reporta tu teléfono se nombra por los lugares dentro del margen de error de tu teléfono: "cerca de { $slot1 }".

journal.window.map.accuracy.explanation.outlines = una visita que reporta tu teléfono se nombra por el edificio, parque o campus en el que estuvo, y por los lugares dentro del margen de error de tu teléfono: "en { $slot1 }", "cerca de { $slot2 }".

journal.window.map.accuracy.offered.names = con precisión de ubicación, una visita que reporta tu teléfono se nombra por los lugares dentro del margen de error de tu teléfono: "cerca de { $slot1 }".

journal.window.map.accuracy.offered.outlines = con precisión de ubicación, una visita que reporta tu teléfono se nombra por el edificio, parque o campus en el que estuvo, y por los lugares dentro del margen de error de tu teléfono: "en { $slot1 }", "cerca de { $slot2 }".

journal.window.map.accuracy.remove.effect = libera { $storage }; los nombres vuelven a la localidad cercana.

journal.window.map.accuracy.status.verification.unavailable = no se pudieron verificar los paquetes de precisión instalados

journal.window.map.accuracy.storage.insufficient = no hay suficiente espacio libre: la precisión de ubicación necesita { $required_storage } libres, más un poco de espacio de sobra.

journal.window.map.accuracy.update.available = la precisión de ubicación está activada. una descarga más reciente está lista desde updates.solstone.app: { $size }.

journal.window.map.area.label.at.place = en{ $slot1 }{ $name }

journal.window.map.area.label.inside = en{ $slot1 }{ $place }

journal.window.map.area.label.inside.qualified = en{ $slot1 }{ $place }{ $slot2 }{ $qualifier }

journal.window.map.area.label.near = cerca de{ $slot1 }{ $place }{ $slot2 }{ $qualifier }

journal.window.map.area.label.near.place = cerca de{ $slot1 }{ $name }

journal.window.map.area.label.qualified = { $region }{ $slot1 }{ $country }

journal.window.map.area.no.town = un lugar sin una localidad nombrada cerca

journal.window.map.area.size = alrededor de{ $slot1 }{ $size }{ $slot2 }km

journal.window.map.attribution.base = localidades de { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · fronteras de Natural Earth

journal.window.map.attribution.outlines = localidades de { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · fronteras de Natural Earth · lugares y contornos de { $overture_start }Overture Maps{ $overture_end }

journal.window.map.attribution.places = localidades de { $geonames_start }GeoNames{ $geonames_end } ({ $licence_start }CC BY 4.0{ $licence_end }) · fronteras de Natural Earth · lugares de { $overture_start }Overture Maps{ $overture_end }

journal.window.map.button.label.clear = limpiar

journal.window.map.button.label.download.again = descargar de nuevo

journal.window.map.button.label.fit = ajustar

journal.window.map.button.label.remove.location.accuracy = quitar la precisión de ubicación

journal.window.map.button.label.save = guardar

journal.window.map.button.label.show.on.the.map = mostrar en el mapa

journal.window.map.button.label.update = actualizar

journal.window.map.calendar.day.name =
    { $state ->
        [stops] { $date }, paradas
        [elsewhere] { $date }, paradas en otra parte: { $places }
        [no-stops] { $date }, sin paradas
        [no-location] { $date }, sin ubicación guardada
        [unreadable] { $date }, no se pudo leer la ubicación
        [kept] { $date }, ubicación guardada
       *[unknown] { $date }, { $state_label }
    }

journal.window.map.calendar.day.name.selected =
    { $state ->
        [stops] { $date }, paradas, tiene una parada en el área elegida
        [elsewhere] { $date }, paradas en otra parte: { $places }, tiene una parada en el área elegida
        [no-stops] { $date }, sin paradas, tiene una parada en el área elegida
        [no-location] { $date }, sin ubicación guardada, tiene una parada en el área elegida
        [unreadable] { $date }, no se pudo leer la ubicación, tiene una parada en el área elegida
        [kept] { $date }, ubicación guardada, tiene una parada en el área elegida
       *[unknown] { $date }, { $state_label }, tiene una parada en el área elegida
    }

journal.window.map.calendar.group.accessible.name = días de{ $slot1 }{ $month }. usa las teclas de flecha para moverte entre los días.

journal.window.map.calendar.location.kept = ubicación guardada

journal.window.map.calendar.location.none = sin ubicación guardada

journal.window.map.calendar.stops.elsewhere = paradas en otra parte

journal.window.map.calendar.stops.here = paradas aquí

journal.window.map.calendar.stops.none = sin paradas

journal.window.map.count.area =
    { $quantity ->
        [one] { $count } área
        [many] { $count } áreas
       *[other] { $count } áreas
    }

journal.window.map.count.day =
    { $quantity ->
        [one] { $count } día
        [many] { $count } días
       *[other] { $count } días
    }

journal.window.map.count.night =
    { $quantity ->
        [one] { $count } noche
        [many] { $count } noches
       *[other] { $count } noches
    }

journal.window.map.count.reading =
    { $quantity ->
        [one] { $count } lectura
        [many] { $count } lecturas
       *[other] { $count } lecturas
    }

journal.window.map.count.record =
    { $quantity ->
        [one] { $count } registro
        [many] { $count } registros
       *[other] { $count } registros
    }

journal.window.map.count.visit =
    { $quantity ->
        [one] { $count } visita
        [many] { $count } visitas
       *[other] { $count } visitas
    }

journal.window.map.date.long = { $weekday },{ $slot1 }{ $day }{ $slot2 }de { $month }

journal.window.map.date.short = { $day } de{ $slot1 }{ $month }

journal.window.map.date.short.weekday = { $weekday }{ $slot1 }{ $date }

journal.window.map.date.short.weekday.comma = { $weekday },{ $slot1 }{ $date }

journal.window.map.date.time = { $day }{ $slot1 }{ $clock }

journal.window.map.date.time.range = { $first }{ $slot1 }a{ $slot2 }{ $last }

journal.window.map.day.location.none = tu diario no tiene ubicación de { $date }. la ubicación puede haber estado en pausa o desactivada, o puede que el teléfono no haya conectado con tu diario. tu diario no puede saber cuál.

journal.window.map.device.android.phone = teléfono { $slot1 }

journal.window.map.device.android.tablet = tablet { $slot1 }

journal.window.map.device.detail.tier.balanced = nivel de detalle en la app de solstone: { $level_start }lugares + idas y venidas{ $level_end }.

journal.window.map.device.detail.tier.full = nivel de detalle en la app de solstone: { $level_start }la imagen completa{ $level_end }.

journal.window.map.device.detail.tier.light = nivel de detalle en la app de solstone: { $level_start }solo lugares{ $level_end }.

journal.window.map.device.from = de{ $slot1 }{ $device }

journal.window.map.device.from.android.phone = de tu teléfono android

journal.window.map.device.from.android.phone.named = de tu teléfono android ({ $device_name })

journal.window.map.device.from.android.tablet = de tu tablet android

journal.window.map.device.from.android.tablet.named = de tu tablet android ({ $device_name })

journal.window.map.device.from.ipad = de tu ipad

journal.window.map.device.from.ipad.named = de tu ipad ({ $device_name })

journal.window.map.device.from.iphone = de tu iphone

journal.window.map.device.from.iphone.named = de tu iphone ({ $device_name })

journal.window.map.device.from.phone = de tu teléfono

journal.window.map.device.from.phone.named = de tu teléfono ({ $device_name })

journal.window.map.device.from.tablet = de tu tableta

journal.window.map.device.from.tablet.named = de tu tablet ({ $device_name })

journal.window.map.device.from.watch = de tu reloj

journal.window.map.device.from.watch.named = de tu reloj ({ $device_name })

journal.window.map.device.interrupted.android.phone =
    { $quantity ->
        [one] tu teléfono android dijo que la ubicación se interrumpió una vez en este día.
        [many] tu teléfono android dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu teléfono android dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.android.phone.twice = tu teléfono android dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.interrupted.android.tablet =
    { $quantity ->
        [one] tu tablet android dijo que la ubicación se interrumpió una vez en este día.
        [many] tu tablet android dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu tablet android dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.android.tablet.twice = tu tablet android dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.interrupted.ipad =
    { $quantity ->
        [one] tu ipad dijo que la ubicación se interrumpió una vez en este día.
        [many] tu ipad dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu ipad dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.ipad.twice = tu ipad dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.interrupted.iphone =
    { $quantity ->
        [one] tu iphone dijo que la ubicación se interrumpió una vez en este día.
        [many] tu iphone dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu iphone dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.iphone.twice = tu iphone dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.interrupted.phone =
    { $quantity ->
        [one] tu teléfono dijo que la ubicación se interrumpió una vez en este día.
        [many] tu teléfono dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu teléfono dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.phone.twice = tu teléfono dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.interrupted.tablet =
    { $quantity ->
        [one] tu tablet dijo que la ubicación se interrumpió una vez en este día.
        [many] tu tablet dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu tablet dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.tablet.twice = tu tablet dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.interrupted.unknown =
    { $quantity ->
        [one] tu { $device } dijo que la ubicación se interrumpió una vez en este día.
        [many] tu { $device } dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu { $device } dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.unknown.twice = tu { $device } dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.interrupted.watch =
    { $quantity ->
        [one] tu reloj dijo que la ubicación se interrumpió una vez en este día.
        [many] tu reloj dijo que la ubicación se interrumpió { $count } veces en este día.
       *[other] tu reloj dijo que la ubicación se interrumpió { $count } veces en este día.
    }

journal.window.map.device.interrupted.watch.twice = tu reloj dijo que la ubicación se interrumpió dos veces en este día.

journal.window.map.device.phone = teléfono

journal.window.map.device.possessive = tu{ $slot1 }{ $device }

journal.window.map.device.possessive.named = tu{ $slot1 }{ $device }{ $slot2 }({ $name })

journal.window.map.device.precision.off = la ubicación precisa está desactivada para solstone en tu { $device }, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.precision.off.android.phone = la ubicación precisa está desactivada para solstone en tu teléfono android, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.precision.off.android.tablet = la ubicación precisa está desactivada para solstone en tu tablet android, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.precision.off.ipad = la ubicación precisa está desactivada para solstone en tu ipad, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.precision.off.iphone = la ubicación precisa está desactivada para solstone en tu iphone, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.precision.off.phone = la ubicación precisa está desactivada para solstone en tu teléfono, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.precision.off.tablet = la ubicación precisa está desactivada para solstone en tu tablet, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.precision.off.watch = la ubicación precisa está desactivada para solstone en tu reloj, así que sus lecturas nombran la localidad, no el edificio.

journal.window.map.device.stay.android.phone = una estancia que tu diario calculó a partir de lecturas de tu teléfono android

journal.window.map.device.stay.android.phone.named = una estancia que tu diario calculó a partir de lecturas de tu teléfono android ({ $device_name })

journal.window.map.device.stay.android.tablet = una estancia que tu diario calculó a partir de lecturas de tu tablet android

journal.window.map.device.stay.android.tablet.named = una estancia que tu diario calculó a partir de lecturas de tu tablet android ({ $device_name })

journal.window.map.device.stay.ipad = una estancia que tu diario calculó a partir de lecturas de tu ipad

journal.window.map.device.stay.ipad.named = una estancia que tu diario calculó a partir de lecturas de tu ipad ({ $device_name })

journal.window.map.device.stay.iphone = una estancia que tu diario calculó a partir de lecturas de tu iphone

journal.window.map.device.stay.iphone.named = una estancia que tu diario calculó a partir de lecturas de tu iphone ({ $device_name })

journal.window.map.device.stay.phone = una estancia que tu diario calculó a partir de lecturas de tu teléfono

journal.window.map.device.stay.phone.named = una estancia que tu diario calculó a partir de lecturas de tu teléfono ({ $device_name })

journal.window.map.device.stay.tablet = una estancia que tu diario calculó a partir de lecturas de tu tablet

journal.window.map.device.stay.tablet.named = una estancia que tu diario calculó a partir de lecturas de tu tablet ({ $device_name })

journal.window.map.device.stay.watch = una estancia que tu diario calculó a partir de lecturas de tu reloj

journal.window.map.device.stay.watch.named = una estancia que tu diario calculó a partir de lecturas de tu reloj ({ $device_name })

journal.window.map.device.tablet = tablet

journal.window.map.device.visit.android.phone = una visita que reportó tu teléfono android

journal.window.map.device.visit.android.phone.named = una visita que reportó tu teléfono android ({ $device_name })

journal.window.map.device.visit.android.tablet = una visita que reportó tu tablet android

journal.window.map.device.visit.android.tablet.named = una visita que reportó tu tablet android ({ $device_name })

journal.window.map.device.visit.ipad = una visita que reportó tu ipad

journal.window.map.device.visit.ipad.named = una visita que reportó tu ipad ({ $device_name })

journal.window.map.device.visit.iphone = una visita que reportó tu iphone

journal.window.map.device.visit.iphone.named = una visita que reportó tu iphone ({ $device_name })

journal.window.map.device.visit.phone = una visita que reportó tu teléfono

journal.window.map.device.visit.phone.named = una visita que reportó tu teléfono ({ $device_name })

journal.window.map.device.visit.tablet = una visita que reportó tu tablet

journal.window.map.device.visit.tablet.named = una visita que reportó tu tablet ({ $device_name })

journal.window.map.device.visit.watch = una visita que reportó tu reloj

journal.window.map.device.visit.watch.named = una visita que reportó tu reloj ({ $device_name })

journal.window.map.disclosure.label.how.places.are.worked.out = cómo se calculan los lugares

journal.window.map.disclosure.label.how.the.month.is.drawn = cómo se dibuja el mes

journal.window.map.disclosure.label.how.this.is.drawn = cómo se dibuja esto

journal.window.map.disclosure.label.licences = licencias

journal.window.map.disclosure.label.why.these.words = por qué estas palabras

journal.window.map.entry.area.town.reason = { $town } es la localidad que mejor representa esta área, a { $distance_km } km de distancia.

journal.window.map.entry.lines.unreadable =
    { $quantity ->
        [one] { $count } línea que tu diario no pudo leer
        [many] { $count } líneas que tu diario no pudo leer
       *[other] { $count } líneas que tu diario no pudo leer
    }

journal.window.map.entry.margin.within = toda esa área está dentro de { $area }.

journal.window.map.entry.open.arrival = hasta { $departure }, no se conoce la llegada

journal.window.map.entry.open.departure = desde { $arrival }, sin salida reportada

journal.window.map.entry.precision.approximate = aproximada

journal.window.map.entry.readings.within = cada lectura aquí, con su margen de error, está dentro de { $area }.

journal.window.map.entry.reported.visit = una visita que{ $slot1 }{ $device }{ $slot2 }reportó

journal.window.map.entry.stay.margin.inside.place = estas lecturas, con sus márgenes, quedan todas dentro de { $margin_meters } m de su centro, y todo eso está dentro de { $place }.

journal.window.map.entry.stay.margin.near.place = { $place } está a { $distance_meters } m; estas lecturas, con sus márgenes, quedan todas dentro de { $margin_meters } m de su centro.

journal.window.map.entry.stay.margin.near.place.category = { $place } ({ $category }) está a { $distance_meters } m; estas lecturas, con sus márgenes, quedan todas dentro de { $margin_meters } m de su centro.

journal.window.map.entry.stay.precise.reason =
    { $quantity ->
        [one] { $count } lectura precisa está listada para esta estancia.
        [many] tu diario calculó esta estancia a partir de { $count } lecturas precisas que se mantuvieron dentro de 100 m de la primera de ellas durante al menos cinco minutos. sus tiempos son los de la primera y la última de ellas.
       *[other] tu diario calculó esta estancia a partir de { $count } lecturas precisas que se mantuvieron dentro de 100 m de la primera de ellas durante al menos cinco minutos. sus tiempos son los de la primera y la última de ellas.
    }

journal.window.map.entry.stay.recorded.device = una estancia que tu diario calculó a partir de lecturas de { $device }

journal.window.map.entry.stay.within = estas lecturas, con sus márgenes, quedan todas dentro de { $area }.

journal.window.map.entry.time.arrival = llegó{ $slot1 }{ $time }

journal.window.map.entry.time.departure = salió{ $slot1 }{ $time }

journal.window.map.entry.time.missing.arrival = no se conoce la llegada

journal.window.map.entry.time.missing.departure = sin salida reportada

journal.window.map.entry.transcripts.segments =
    { $quantity ->
        [one] abrir en transcripciones: { $count } segmento
        [many] abrir en transcripciones: { $count } segmentos
       *[other] abrir en transcripciones: { $count } segmentos
    }

journal.window.map.entry.visit.margin.inside.place = tu teléfono te ubicó dentro de { $margin_meters } m, y todo eso está dentro de { $place }.

journal.window.map.entry.visit.margin.near.place = { $place } está a { $distance_meters } m; tu teléfono te ubicó dentro de { $margin_meters } m.

journal.window.map.entry.visit.margin.near.place.category = { $place } ({ $category }) está a { $distance_meters } m; tu teléfono te ubicó dentro de { $margin_meters } m.

journal.window.map.entry.visit.margin.too.wide = tu teléfono te ubicó dentro de { $margin_meters } m, demasiado amplio para nombrar un edificio o los lugares cercanos.

journal.window.map.entry.visit.within = la posición de tu teléfono, con su margen de error, está dentro de { $area }.

journal.window.map.field.label.call.it = llamarlo

journal.window.map.field.label.hide.this.place = ocultar este lugar

journal.window.map.field.label.name = nombre

journal.window.map.heading.couldn.t.open.this.day.s = no se pudo abrir la ubicación de este día.

journal.window.map.heading.no.location.in.your.journal.yet = todavía no hay ubicación en tu diario

journal.window.map.heading.no.location.kept.for.this.day = sin ubicación guardada para este día

journal.window.map.label.about.km = alrededor de 1 km

journal.window.map.leaflet.zoom.in = Acercar

journal.window.map.leaflet.zoom.out = Alejar

journal.window.map.link.label.next.month = mes siguiente

journal.window.map.link.label.open.in.transcripts = abrir en transcripciones

journal.window.map.link.label.open.the.day = abrir el día

journal.window.map.link.label.place = lugar

journal.window.map.link.label.places = lugares

journal.window.map.link.label.previous.month = mes anterior

journal.window.map.link.label.the.day = el día

journal.window.map.link.label.the.week.after = la semana siguiente

journal.window.map.link.label.the.week.before = la semana anterior

journal.window.map.link.label.this.month = este mes

journal.window.map.link.label.this.week = esta semana

journal.window.map.list.conjunction = { $leading }{ $slot1 }y{ $slot2 }{ $last }

journal.window.map.month.area.around = alrededor de{ $slot1 }{ $name }

journal.window.map.month.area.clipped = solo la parte dentro de { $month } cuenta.

journal.window.map.month.area.days.in.month =
    { $quantity ->
        [one] { $count } día este mes
        [many] { $count } días este mes
       *[other] { $count } días este mes
    }

journal.window.map.month.area.days.ratio = { $days_with_stops } de { $days_in_month }

journal.window.map.month.area.days.show.all =
    { $quantity ->
        [one] mostrar { $count } día
        [many] mostrar todos los { $count } días
       *[other] mostrar todos los { $count } días
    }

journal.window.map.month.area.distance.first =
    { $direction ->
       *[east] { $distance } km al este de la primera área de la lista
        [north-east] { $distance } km al noreste de la primera área de la lista
        [north] { $distance } km al norte de la primera área de la lista
        [north-west] { $distance } km al noroeste de la primera área de la lista
        [west] { $distance } km al oeste de la primera área de la lista
        [south-west] { $distance } km al suroeste de la primera área de la lista
        [south] { $distance } km al sur de la primera área de la lista
        [south-east] { $distance } km al sureste de la primera área de la lista
    }

journal.window.map.month.area.distance.month =
    { $direction ->
       *[east] { $distance } km al este del área más concurrida
        [north-east] { $distance } km al noreste del área más concurrida
        [north] { $distance } km al norte del área más concurrida
        [north-west] { $distance } km al noroeste del área más concurrida
        [west] { $distance } km al oeste del área más concurrida
        [south-west] { $distance } km al suroeste del área más concurrida
        [south] { $distance } km al sur del área más concurrida
        [south-east] { $distance } km al sureste del área más concurrida
    }

journal.window.map.month.area.distance.trip =
    { $direction ->
       *[east] { $distance } km al este del área más concurrida de este viaje
        [north-east] { $distance } km al noreste del área más concurrida de este viaje
        [north] { $distance } km al norte del área más concurrida de este viaje
        [north-west] { $distance } km al noroeste del área más concurrida de este viaje
        [west] { $distance } km al oeste del área más concurrida de este viaje
        [south-west] { $distance } km al suroeste del área más concurrida de este viaje
        [south] { $distance } km al sur del área más concurrida de este viaje
        [south-east] { $distance } km al sureste del área más concurrida de este viaje
    }

journal.window.map.month.area.live.days =
    { $quantity ->
        [one] { $name }, { $count } día este mes
        [many] { $name }, { $count } días este mes
       *[other] { $name }, { $count } días este mes
    }

journal.window.map.month.area.open.stops =
    { $quantity ->
        [one] 1 visita no tiene su llegada o su salida, así que su tiempo no está en las horas ni en las noches. su día sí cuenta.
        [many] { $count } visitas no tienen una llegada o una salida, así que su tiempo no está en las horas ni en las noches. sus días sí cuentan.
       *[other] { $count } visitas no tienen una llegada o una salida, así que su tiempo no está en las horas ni en las noches. sus días sí cuentan.
    }

journal.window.map.month.area.size = un área de { $size } de ancho

journal.window.map.month.areas.list.region =
    { $quantity ->
        [one] { $count } área { $area }, primero las de más días.
        [many] { $count } áreas { $area }, primero las de más días.
       *[other] { $count } áreas { $area }, primero las de más días.
    }

journal.window.map.month.areas.list.region.home =
    { $quantity ->
        [one] { $count } área alrededor de { $area }, primero las de más días.
        [many] { $count } áreas alrededor de { $area }, primero las de más días.
       *[other] { $count } áreas alrededor de { $area }, primero las de más días.
    }

journal.window.map.month.areas.list.region.shared =
    { $quantity ->
        [one] { $count } área { $area }, toda en { $region }, primero las de más días.
        [many] { $count } áreas { $area }, todas en { $region }, primero las de más días.
       *[other] { $count } áreas { $area }, todas en { $region }, primero las de más días.
    }

journal.window.map.month.areas.list.region.shared.home =
    { $quantity ->
        [one] { $count } área alrededor de { $area }, toda en { $region }, primero las de más días.
        [many] { $count } áreas alrededor de { $area }, todas en { $region }, primero las de más días.
       *[other] { $count } áreas alrededor de { $area }, todas en { $region }, primero las de más días.
    }

journal.window.map.month.areas.list.trip =
    { $quantity ->
        [one] { $count } área en el viaje del { $date }, primero las de más días.
        [many] { $count } áreas en el viaje del { $date }, primero las de más días.
       *[other] { $count } áreas en el viaje del { $date }, primero las de más días.
    }

journal.window.map.month.areas.list.trip.shared =
    { $quantity ->
        [one] { $count } área en el viaje del { $date }, toda en { $region }, primero las de más días.
        [many] { $count } áreas en el viaje del { $date }, todas en { $region }, primero las de más días.
       *[other] { $count } áreas en el viaje del { $date }, todas en { $region }, primero las de más días.
    }

journal.window.map.month.areas.live.zoom =
    { $quantity ->
        [one] con más zoom en { $count } área
        [many] con más zoom en { $count } áreas
       *[other] con más zoom en { $count } áreas
    }

journal.window.map.month.areas.outlined =
    { $quantity ->
        [one] { $count } área con contorno
        [many] { $count } áreas con contorno
       *[other] { $count } áreas con contorno
    }

journal.window.map.month.areas.outside.show.all =
    { $quantity ->
        [one] { $count } área fuera del mapa · mostrar todo
        [many] { $count } áreas fuera del mapa · mostrar todo
       *[other] { $count } áreas fuera del mapa · mostrar todo
    }

journal.window.map.month.areas.show.all =
    { $quantity ->
        [one] mostrar { $count } área
        [many] mostrar todas las { $count } áreas
       *[other] mostrar todas las { $count } áreas
    }

journal.window.map.month.choice.trip = en el viaje{ $slot2 }del{ $slot1 }{ $date }

journal.window.map.month.day.location.none = tu diario no tiene ubicación de{ $slot1 }{ $date }. la ubicación puede haber estado en pausa o desactivada, o el teléfono puede no haber conectado con tu diario. tu diario no puede saber cuál.

journal.window.map.month.day.stay.readings =
    { $quantity ->
        [one] { $count } lectura de tu teléfono está listada para esta estancia
        [many] una estancia que tu diario calculó a partir de { $count } de las lecturas de tu teléfono
       *[other] una estancia que tu diario calculó a partir de { $count } de las lecturas de tu teléfono
    }

journal.window.map.month.days.show.fewer = mostrar menos días

journal.window.map.month.explanation.midnight = solo el tiempo dentro de{ $slot1 }{ $month }{ $slot2 }cuenta. una visita que empezó el{ $slot5 }{ $day } de{ $slot4 }{ $previous }{ $slot3 }cuenta desde la medianoche.

journal.window.map.month.heading = { $month }{ $slot1 }{ $year }

journal.window.map.month.live.areas.all = mostrando todas las áreas

journal.window.map.month.live.choice.none = nada elegido

journal.window.map.month.live.fit = mapa ajustado

journal.window.map.month.live.showing.choice = mostrando{ $slot1 }{ $name }

journal.window.map.month.live.showing.home = mostrando alrededor de { $area }

journal.window.map.month.live.zoom.amount = zoom de{ $slot1 }{ $zoom }

journal.window.map.month.location.gaps =
    { $quantity ->
        [one] no se guardó la ubicación en { $count } día.
        [many] no se guardó la ubicación en { $count } días.
       *[other] no se guardó la ubicación en { $count } días.
    }

journal.window.map.month.map.area.summary =
    { $shaded_quantity ->
        [one] mapa de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho. la misma área está listada en áreas de este mes.
        [many] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho. las mismas áreas están listadas en áreas de este mes.
       *[other] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho. las mismas áreas están listadas en áreas de este mes.
    }

journal.window.map.month.map.area.summary.home =
    { $shaded_quantity ->
        [one] mapa del área alrededor de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho. la misma área está listada en áreas de este mes.
        [many] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho. las mismas áreas están listadas en áreas de este mes.
       *[other] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho. las mismas áreas están listadas en áreas de este mes.
    }

journal.window.map.month.map.group.summary =
    { $shaded_quantity ->
        [one] { $area_quantity ->
        [one] mapa de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho, que guarda el { $areas } área listada en áreas de este mes.
        [many] mapa de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho, que guarda las { $areas } áreas listadas en áreas de este mes.
       *[other] mapa de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho, que guarda las { $areas } áreas listadas en áreas de este mes.
    }
        [many] { $area_quantity ->
        [one] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan el { $areas } área listada en áreas de este mes.
        [many] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
       *[other] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
    }
       *[other] { $area_quantity ->
        [one] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan el { $areas } área listada en áreas de este mes.
        [many] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
       *[other] mapa de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
    }
    }

journal.window.map.month.map.group.summary.home =
    { $shaded_quantity ->
        [one] { $area_quantity ->
        [one] mapa del área alrededor de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho, que guarda el { $areas } área listada en áreas de este mes.
        [many] mapa del área alrededor de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho, que guarda las { $areas } áreas listadas en áreas de este mes.
       *[other] mapa del área alrededor de { $area }: { $shaded_areas } área sombreada, de { $size } de ancho, que guarda las { $areas } áreas listadas en áreas de este mes.
    }
        [many] { $area_quantity ->
        [one] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan el { $areas } área listada en áreas de este mes.
        [many] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
       *[other] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
    }
       *[other] { $area_quantity ->
        [one] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan el { $areas } área listada en áreas de este mes.
        [many] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
       *[other] mapa del área alrededor de { $area }: { $shaded_areas } áreas sombreadas, cada una de { $size } de ancho, que guardan las { $areas } áreas listadas en áreas de este mes.
    }
    }

journal.window.map.month.map.nothing.drawn = aquí no hay nada dibujado

journal.window.map.month.readings.undrawn =
    { $quantity ->
        [one] sin dibujar: { $count } lectura.
        [many] sin dibujar: { $count } lecturas.
       *[other] sin dibujar: { $count } lecturas.
    }

journal.window.map.month.stops.calendar =
    { $stops_quantity ->
        [one] { $days_with_stops } de { $days_in_month } días tiene paradas
        [many] { $days_with_stops } de { $days_in_month } días tienen paradas
       *[other] { $days_with_stops } de { $days_in_month } días tienen paradas
    }

journal.window.map.month.stops.glance =
    { $stops_quantity ->
        [one] { $days_with_stops } de { $days_in_month } días tiene paradas.
        [many] { $days_with_stops } de { $days_in_month } días tienen paradas.
       *[other] { $days_with_stops } de { $days_in_month } días tienen paradas.
    }

journal.window.map.month.stops.glance.home.days =
    { $stops_quantity ->
        [one] { $days_with_stops } de { $days_in_month } días tiene paradas: { $home_days } alrededor de { $home_area }.
        [many] { $days_with_stops } de { $days_in_month } días tienen paradas: { $home_days } alrededor de { $home_area }.
       *[other] { $days_with_stops } de { $days_in_month } días tienen paradas: { $home_days } alrededor de { $home_area }.
    }

journal.window.map.month.stops.glance.trips.days =
    { $trip_quantity ->
        [one] { $days_with_stops } de { $days_in_month } días { $stops_quantity ->
            [one] tiene
            [many] tienen
           *[other] tienen
        } paradas: { $home_days } alrededor de { $home_area }, y { $trips } viaje a más de 50 km.
        [many] { $days_with_stops } de { $days_in_month } días { $stops_quantity ->
            [one] tiene
            [many] tienen
           *[other] tienen
        } paradas: { $home_days } alrededor de { $home_area }, y { $trips } viajes a más de 50 km.
       *[other] { $days_with_stops } de { $days_in_month } días { $stops_quantity ->
            [one] tiene
            [many] tienen
           *[other] tienen
        } paradas: { $home_days } alrededor de { $home_area }, y { $trips } viajes a más de 50 km.
    }

journal.window.map.month.stops.undrawn.hidden =
    { $quantity ->
        [one] un lugar que ocultaste
        [many] { $count } lugares que ocultaste
       *[other] { $count } lugares que ocultaste
    }

journal.window.map.month.stops.undrawn.wide =
    { $quantity ->
        [one] { $count } parada con un margen de error de más de 500 m o sin reportar
        [many] { $count } paradas con un margen de error de más de 500 m o sin reportar
       *[other] { $count } paradas con un margen de error de más de 500 m o sin reportar
    }

journal.window.map.month.strip.faded.bar = a una barra que se desvanece le falta la llegada o la salida.

journal.window.map.month.strip.visits.and.stays = las barras llenas son visitas que reportó tu teléfono; las barras con contorno son estancias que calculó tu diario.

journal.window.map.month.time.all.day = todo el día

journal.window.map.month.time.from = desde{ $slot1 }{ $time }

journal.window.map.month.time.until = hasta{ $slot1 }{ $time }

journal.window.map.month.zoom.notice = con más zoom.

journal.window.map.months.label.april = abril

journal.window.map.months.label.august = agosto

journal.window.map.months.label.december = diciembre

journal.window.map.months.label.february = febrero

journal.window.map.months.label.january = enero

journal.window.map.months.label.july = julio

journal.window.map.months.label.june = junio

journal.window.map.months.label.march = marzo

journal.window.map.months.label.may = mayo

journal.window.map.months.label.november = noviembre

journal.window.map.months.label.october = octubre

journal.window.map.months.label.september = septiembre

journal.window.map.navigation.go.day = ir a { $date }

journal.window.map.navigation.go.month = ir a { $month } { $year }

journal.window.map.option.label.home = casa

journal.window.map.option.label.nothing = nada

journal.window.map.option.label.work = trabajo

journal.window.map.place.named.link = un lugar que nombraste "{ $name }"

journal.window.map.place.named.reason = nombraste este lugar "{ $name }".

journal.window.map.place.recorded.dates =
    { $quantity ->
        [one] visitas o estancias aquí en { $count } fecha.
        [many] visitas o estancias aquí en { $count } fechas.
       *[other] visitas o estancias aquí en { $count } fechas.
    }

journal.window.map.place.recorded.dates.range =
    { $quantity ->
        [one] visitas o estancias aquí en { $count } fecha, { $first } a { $last }.
        [many] visitas o estancias aquí en { $count } fechas, { $first } a { $last }.
       *[other] visitas o estancias aquí en { $count } fechas, { $first } a { $last }.
    }

journal.window.map.place.recorded.dates.single =
    { $quantity ->
        [one] visitas o estancias aquí en { $count } fecha, { $date }.
        [many] visitas o estancias aquí en { $count } fechas, { $date }.
       *[other] visitas o estancias aquí en { $count } fechas, { $date }.
    }

journal.window.map.place.recorded.nights =
    { $quantity ->
        [one] en { $count } noche, una visita o estancia aquí cubre al menos dos horas entre las 22:00 y las 06:00.
        [many] en { $count } noches, una visita o estancia aquí cubre al menos dos horas entre las 22:00 y las 06:00.
       *[other] en { $count } noches, una visita o estancia aquí cubre al menos dos horas entre las 22:00 y las 06:00.
    }

journal.window.map.place.visibility.hidden = · oculto

journal.window.map.response.accuracy.busy = la precisión de ubicación ya se está descargando.

journal.window.map.response.accuracy.busy.detail = Hay una descarga en curso

journal.window.map.response.accuracy.download.failed = no se pudo descargar la precisión de ubicación.

journal.window.map.response.accuracy.not.offered = esta versión de tu diario no tiene descarga de precisión de ubicación.

journal.window.map.response.accuracy.not.offered.detail = No se ofrece nada

journal.window.map.response.accuracy.remove.failed = no se pudo quitar la precisión de ubicación.

journal.window.map.response.area.unavailable = no se pudieron leer los nombres de lugares que vienen con tu diario.

journal.window.map.response.day.invalid = no se pudo usar ese día.

journal.window.map.response.day.invalid.detail = No se encontró el día

journal.window.map.response.edit.alias = { $slot1 } debe ser { $slot2 }, { $slot3 } o { $slot4 }

journal.window.map.response.edit.body = El cuerpo debe ser un objeto { $slot1 }

journal.window.map.response.edit.failed = no se pudo guardar ese cambio.

journal.window.map.response.edit.hidden = { $slot1 } debe ser { $slot2 } o { $slot3 }

journal.window.map.response.edit.invalid = no se pudo usar ese cambio.

journal.window.map.response.edit.name = { $slot1 } debe ser una cadena o { $slot2 }

journal.window.map.response.month.invalid = no se pudo usar ese mes.

journal.window.map.response.month.invalid.detail = El mes debe ser { $slot1 }

journal.window.map.response.place.unknown = ese lugar no está en tu diario.

journal.window.map.response.place.unknown.detail = Lugar no encontrado

journal.window.map.section.heading.areas.this.month = áreas de este mes

journal.window.map.section.heading.day.by.day = día a día

journal.window.map.section.heading.deleting.any.of.this = eliminar cualquiera de esto

journal.window.map.section.heading.edit.this.place = editar este lugar

journal.window.map.section.heading.location.accuracy = precisión de ubicación

journal.window.map.section.heading.the.day.in.order = el día, en orden

journal.window.map.section.heading.the.map = el mapa

journal.window.map.section.heading.the.month.at.a.glance = el mes de un vistazo

journal.window.map.section.heading.visits.and.stays.here = visitas y estancias aquí

journal.window.map.status.text.couldn.t.open.this.month.s = no se pudo abrir la ubicación de este mes.

journal.window.map.text.loading = cargando…

journal.window.map.text.loading.this.day.s.location = cargando la ubicación de este día…

journal.window.map.text.loading.your.journal.s.location = cargando la ubicación de tu diario…

journal.window.map.visible.text.a.hidden.place.is.off.the = un lugar oculto está fuera del mapa, y lo que tu diario escribe sobre dónde estuviste cada día lo deja fuera: en la búsqueda, para los agentes conectados y en las actividades. los resúmenes diarios nuevos también lo dejan fuera.

journal.window.map.visible.text.a.night.counts.in.an.area = una noche cuenta en un área cuando una visita ahí cubre al menos dos horas entre las 22:00 y las 06:00.

journal.window.map.visible.text.a.place.is.a.spot.your = un lugar es un punto donde tu teléfono reportó una visita, o una estancia, en dos fechas distintas, lo bastante cerca para ser un solo punto. la ubicación aproximada nunca forma un lugar.

journal.window.map.visible.text.a.place.lasts.as.long.as = un lugar dura lo que dura un segmento del que salió. elimina esos segmentos en transcripciones y el lugar, y su nombre, también se van.

journal.window.map.visible.text.a.place.starts.from.visits.or = un lugar empieza a partir de visitas o estancias en dos o más fechas, todas dentro de 50 m entre sí y a más de 100 m de cualquier otro lugar. las posteriores dentro de 50 m de él se le unen. una estancia o visita con un margen de error de más de 50 m nunca forma uno ni se une a uno.

journal.window.map.visible.text.a.place.you.hid.isn.t = un lugar que ocultaste no se dibuja. sus visitas se siguen mostrando en la lista de cada día.

journal.window.map.visible.text.a.stay.is.worked.out.from = una estancia se calculó a partir de lecturas precisas, ninguna a más de cinco minutos de la otra, que se mantuvieron dentro de 100 m de la primera de ellas durante al menos cinco minutos. las lecturas aproximadas nunca forman una estancia. sus horas son su primera y su última lectura.

journal.window.map.visible.text.a.stop.at.one.of.your = una parada en uno de tus lugares se dibuja donde está el lugar, así que un lugar nunca se divide entre dos áreas. una parada cuyo margen de error no se reportó, o es de más de 500 m, no se dibuja, y tampoco el tiempo entre paradas.

journal.window.map.visible.text.also.in.this.area = también en esta área

journal.window.map.visible.text.checking.location.accuracy.before.using.it = revisando la precisión de ubicación antes de usarla. puedes seguir usando tu diario.

journal.window.map.visible.text.days.here = días aquí

journal.window.map.visible.text.days.with.a.stop = días con una parada

journal.window.map.visible.text.each.area.is.about.km.across = cada área mide alrededor de 1 km de ancho y guarda las paradas en ella: las visitas que reportó tu teléfono, y las estancias que calculó tu diario.

journal.window.map.visible.text.every.record.above.opens.in.transcripts = cada registro de arriba se abre en transcripciones, donde eliminas sus segmentos del mismo modo que cualquier otro. un segmento eliminado sale de esta vista y de la búsqueda.

journal.window.map.visible.text.filled.bars.are.visits.your.phone = las barras llenas son visitas que reportó tu teléfono; las barras con contorno son estancias que calculó tu diario. cada lectura está en la página de cada día.

journal.window.map.visible.text.for.readings.the.times.are.the = para las lecturas, las horas son la primera y la última lectura de la fila. no dicen cuánto tiempo estuviste en ninguna parte.

journal.window.map.visible.text.hidden = oculto

journal.window.map.visible.text.hours.are.shown.for.one.area = las horas se muestran para un área a la vez, y solo para el tiempo con ambos extremos conocidos. no se suman entre áreas, porque dos reportes pueden ubicar la misma hora en dos áreas.

journal.window.map.visible.text.hours.with.both.ends.known = horas con ambos extremos conocidos

journal.window.map.visible.text.location.accuracy.is.on = la precisión de ubicación está activada.

journal.window.map.visible.text.location.accuracy.is.unavailable = la precisión de ubicación no está disponible.

journal.window.map.visible.text.location.may.have.been.paused.or = la ubicación puede haber estado en pausa o desactivada, o el teléfono puede no haber conectado con tu diario. tu diario no puede saber cuál.

journal.window.map.visible.text.location.was.kept.this.day.but = la ubicación se guardó este día, pero no hay paradas.

journal.window.map.visible.text.named.features.only.streets.not.shown = solo elementos con nombre; no se muestran calles.

journal.window.map.visible.text.nights = noches

journal.window.map.visible.text.no.location.kept.for.this.day = sin ubicación guardada para este día.

journal.window.map.visible.text.no.location.kept.for.this.month = sin ubicación guardada para este mes.

journal.window.map.visible.text.no.places.yet = todavía no hay lugares.

journal.window.map.visible.text.no.stops.to.draw.this.month = no hay paradas para dibujar este mes.

journal.window.map.visible.text.not.drawn.here = no se dibuja aquí:

journal.window.map.visible.text.nothing.drawn.here = · aquí no hay nada dibujado

journal.window.map.visible.text.on.the.map.a.run.of = en el mapa, una serie de lecturas precisas se muestra como un punto en su lectura del medio, así que un día no se lee como una ruta. la lista cuenta cada lectura.

journal.window.map.visible.text.saved = se guardó.

journal.window.map.visible.text.shades.run.from.day.lightest.through = los sombreados van de 1 día, el más claro, pasando por 2 a 3, 4 a 7 y 8 a 14, hasta 15 días o más, el más oscuro.

journal.window.map.visible.text.show.fewer = mostrar menos

journal.window.map.visible.text.stops = paradas

journal.window.map.visible.text.stops.in.order = paradas, en orden

journal.window.map.visible.text.that.change.couldn.t.be.saved = no se pudo guardar ese cambio.

journal.window.map.visible.text.the.shade.counts.days.not.hours = el sombreado cuenta días, no horas. un día cuenta una vez en un área, tenga los reportes que tenga, y una visita cuenta para su día aunque no se conozca su llegada o su salida.

journal.window.map.visible.text.the.stops.your.journal.holds.for = las paradas que tu diario guarda de este mes. cada área se sombrea según cuántos días tienen una parada en ella.

journal.window.map.visible.text.there.s.no.street.map.a = no hay un mapa de calles. un mapa de calles se carga por partes desde un servicio de mapas, y pedir la parte correcta le dice a ese servicio dónde estuviste.

journal.window.map.visible.text.there.s.no.street.map.the = no hay un mapa de calles. los nombres son los que vinieron con tu diario o los que descargaste, y no se le pide nada a ningún servicio de mapas.

journal.window.map.visible.text.to.remove.a.visit.or.stay = para quitar una visita o estancia, abre su día y elimina sus segmentos en transcripciones. un lugar dura lo que dura un segmento del que salió.

journal.window.map.visible.text.what.you.call.a.place.a = lo que llamas un lugar, un nombre o casa o trabajo, se usa en la búsqueda y en el propio pensamiento de tu diario, y un agente que pueda ver transcripciones de todo tu diario puede leerlo.

journal.window.map.visible.text.where = dónde

journal.window.map.visible.text.where.starts.around.the.area.with = "dónde" empieza alrededor del área con más días: cada parada dentro de 50 km de ella. cualquier serie de días con una parada más lejos es un viaje, nombrado por sus fechas.

journal.window.map.visible.text.with.location.accuracy.a.visit.can = con precisión de ubicación, una visita puede decir "cerca" de los lugares dentro del margen de error de tu teléfono. cada registro dice por qué se lee como se lee.

journal.window.map.visible.text.with.location.accuracy.a.visit.can.variant2 = con precisión de ubicación, una visita puede decir "en" un lugar cuando todo el margen de error de tu teléfono está dentro de su contorno, y "cerca" de los lugares dentro de ese margen. "dentro" significa que todo el margen está dentro de un contorno más grande, un barrio o una localidad. cada registro dice por qué se lee como se lee.

journal.window.map.visible.text.without.it.a.visit.is.named = sin eso, una visita se nombra por la localidad cercana.

journal.window.map.visible.text.without.location.accuracy.each.reading.is = sin precisión de ubicación, cada lectura se nombra según la localidad que mejor representa su área, que no siempre es la localidad en la que estuviste. por eso dice "cerca".

journal.window.map.visible.text.you.can.name.a.place.call = puedes nombrar un lugar, llamarlo casa o trabajo, u ocultarlo. ocultarlo mantiene fuera del mapa todo lo que está dentro de 50 m de él, y lo que tu diario escribe sobre dónde estuviste cada día lo deja fuera. la lista del día aquí lo sigue mostrando.

journal.window.map.visible.text.your.journal.couldn.t.read.the = tu diario no pudo leer la ubicación guardada de este día.

journal.window.map.visible.text.your.journal.draws.this.from.the = tu diario dibuja esto a partir de las lecturas que le envió tu teléfono, y de los nombres que vinieron con tu diario o que descargaste. no se le pide nada a ningún servicio de mapas.

journal.window.map.visible.text.your.phone.often.reports.an.arrival = tu teléfono a menudo reporta primero una llegada y después la visita completa. los dos reportes son una visita.

journal.window.map.week.heading = la semana de{ $slot1 }{ $date }

journal.window.map.week.readings.without.stops =
    { $quantity ->
        [one] { $count } lectura, sin visitas ni estancias
        [many] { $count } lecturas, sin visitas ni estancias
       *[other] { $count } lecturas, sin visitas ni estancias
    }

journal.window.map.window.explanation.a.place.in.your.journal = un lugar en tu diario

journal.window.map.window.explanation.a.segment.your.journal.couldn.t = un segmento que tu diario no pudo leer

journal.window.map.window.explanation.a.stay.your.journal.worked.out = una estancia que calculó tu diario

journal.window.map.window.explanation.a.visit.your.phone.reported = una visita que reportó tu teléfono

journal.window.map.window.explanation.approximate.location.each.reading.is.only = ubicación aproximada: cada lectura solo sirve dentro de unos pocos kilómetros, lo bastante para nombrar la localidad, no el edificio.

journal.window.map.window.explanation.choose.a.day.in.the.calendar = elige un día en el calendario.

journal.window.map.window.explanation.choose.an.area.on.the.map = elige un área en el mapa o en la lista, o un día en el calendario.

journal.window.map.window.explanation.each.bar.is.a.visit.your = cada barra es una visita que reportó tu teléfono.

journal.window.map.window.explanation.hidden.from.the.map = oculto del mapa

journal.window.map.window.explanation.in.the.chosen.area = en el área elegida

journal.window.map.window.explanation.location.accuracy.couldn.t.be.downloaded = no se pudo descargar la precisión de ubicación. vuelve a intentarlo en un rato.

journal.window.map.window.explanation.location.accuracy.couldn.t.be.saved = no se pudo guardar la precisión de ubicación en la carpeta de tu diario.

journal.window.map.window.explanation.location.accuracy.failed.its.file.check = la precisión de ubicación no pasó la revisión de su archivo. descárgala de nuevo antes de usarla.

journal.window.map.window.explanation.location.comes.from.the.solstone.app = la ubicación viene de la app de solstone en tu teléfono, una vez que la activas ahí. lo que recibe entra en tu diario y aparece aquí, día a día.

journal.window.map.window.explanation.location.couldn.t.be.read = no se pudo leer la ubicación

journal.window.map.window.explanation.map.fitted.to.these.areas = mapa ajustado a estas áreas

journal.window.map.window.explanation.margin.of.error.over.m.or = margen de error de más de 500 m o sin reportar

journal.window.map.window.explanation.no.named.town.nearby = no hay una localidad con nombre cerca

journal.window.map.window.explanation.open.it.in.transcripts.to.see = ábrelo en transcripciones para verlo o eliminarlo

journal.window.map.window.explanation.the.area.with.the.most.days = el área con más días

journal.window.map.window.explanation.this.trip.s.busiest.area = el área más concurrida de este viaje

journal.window.map.window.explanation.this.version.of.your.journal.has = esta versión de tu diario no tiene descarga de precisión de ubicación.

journal.window.map.window.explanation.tied.for.the.most.days = empate en más días

journal.window.map.window.explanation.where.two.reports.cover.the.same = cuando dos reportes cubren el mismo tiempo, las horas cuentan ese tiempo una sola vez.

journal.window.map.window.explanation.your.journal.couldn.t.read.part = tu diario no pudo leer parte de la ubicación de este día.

journal.window.map.window.explanation.your.watch.gives.only.approximate.location = tu reloj solo da una ubicación aproximada, así que sus lecturas nombran la localidad, no el edificio.
