journal.window.entities.accessible.label.dismiss = వదిలివేయండి

journal.window.entities.accessible.name.close = మూసివేయండి

journal.window.entities.accessible.name.entity.folders = ఎంటిటీ ఫోల్డర్‌లు

journal.window.entities.accessible.name.search.detected.entities = గుర్తించిన ఎంటిటీలను వెతకండి

journal.window.entities.accessible.name.search.entities = ఎంటిటీలను వెతకండి

journal.window.entities.accessible.name.search.entity.cards = ఎంటిటీ కార్డులను వెతకండి

journal.window.entities.accessible.name.sort.entities = ఎంటిటీలను క్రమబద్ధం చేయండి

journal.window.entities.aliases.other.names = ఇలా కూడా: { $aliases }

journal.window.entities.button.detail.reattach = మళ్ళీ జోడించండి

journal.window.entities.button.label.attach.to.facet = ఫాసెట్‌కు జోడించండి

journal.window.entities.button.label.cancel = రద్దు చేయండి

journal.window.entities.button.label.detach = విడదీయండి

journal.window.entities.button.label.hide = దాచండి

journal.window.entities.button.label.next = తర్వాత

journal.window.entities.button.label.previous = మునుపటి

journal.window.entities.button.label.save.changes = మార్పులు సేవ్ చేయండి

journal.window.entities.column.heading.description = వివరణ

journal.window.entities.column.heading.name = పేరు

journal.window.entities.connections.calendar.last.day = { $day }

journal.window.entities.connections.evidence.show.more = { $shown_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $shown } చూపిస్తోంది. మరిన్ని చూపండి
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $shown } చూపిస్తోంది. మరిన్ని చూపండి
}

journal.window.entities.connections.row.attendance = { $total_quantity ->
    [one] 1 ఈవెంట్ కలిసి · { $day }
   *[other] { $total } ఈవెంట్‌లు కలిసి · { $day }
}

journal.window.entities.connections.row.direct = { $total_quantity ->
    [one] 1 క్షణం · { $day }
   *[other] { $total } క్షణాలు · { $day }
}

journal.window.entities.connections.summary = { $direct_quantity ->
    [one] { $direct } నేరుగా ఆధారంతో · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } ఉమ్మడి ఈవెంట్‌ల నుండి మాత్రమే
   *[other] { $direct } నేరుగా ఆధారంతో · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } ఉమ్మడి ఈవెంట్‌ల నుండి మాత్రమే
}

journal.window.entities.connections.summary.with.calendar = { $direct_quantity ->
    [one] { $direct } నేరుగా ఆధారంతో · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } ఉమ్మడి ఈవెంట్‌ల నుండి మాత్రమే · { $calendar_quantity ->
        [one] { $calendar }
       *[other] { $calendar }
    } మీ క్యాలెండర్ నుండి మాత్రమే
   *[other] { $direct } నేరుగా ఆధారంతో · { $shared_quantity ->
        [one] { $shared }
       *[other] { $shared }
    } ఉమ్మడి ఈవెంట్‌ల నుండి మాత్రమే · { $calendar_quantity ->
        [one] { $calendar }
       *[other] { $calendar }
    } మీ క్యాలెండర్ నుండి మాత్రమే
}

journal.window.entities.connections.you.calendar.latest = మీ క్యాలెండర్‌లో మాత్రమే · ఇటీవలి { $day }

journal.window.entities.connections.you.latest.attended.with = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి ఈవెంట్‌లు ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి ఈవెంట్‌లు ({ $day })
}

journal.window.entities.connections.you.latest.co.present = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి ఒకే చోట ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి ఒకే చోట ({ $day })
}

journal.window.entities.connections.you.latest.created = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి సృష్టించిన ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి సృష్టించిన ({ $day })
}

journal.window.entities.connections.you.latest.fallback = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి { $evidence_name } ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి { $evidence_name } ({ $day })
}

journal.window.entities.connections.you.latest.family.of = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి కుటుంబం ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి కుటుంబం ({ $day })
}

journal.window.entities.connections.you.latest.knows = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి తెలుసు ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి తెలుసు ({ $day })
}

journal.window.entities.connections.you.latest.mentioned = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి ప్రస్తావించిన ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి ప్రస్తావించిన ({ $day })
}

journal.window.entities.connections.you.latest.messaged.with = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి సందేశం ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి సందేశం ({ $day })
}

journal.window.entities.connections.you.latest.other = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి సంబంధిత ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి సంబంధిత ({ $day })
}

journal.window.entities.connections.you.latest.party.of = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి పాలుదారు ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి పాలుదారు ({ $day })
}

journal.window.entities.connections.you.latest.reports.to = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి నివేదిస్తుంది ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి నివేదిస్తుంది ({ $day })
}

journal.window.entities.connections.you.latest.scheduled.with = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి షెడ్యూల్ ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి షెడ్యూల్ ({ $day })
}

journal.window.entities.connections.you.latest.spoke.with = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి మాట్లాడిన ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి మాట్లాడిన ({ $day })
}

journal.window.entities.connections.you.latest.uses = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి వాడుతుంది ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి వాడుతుంది ({ $day })
}

journal.window.entities.connections.you.latest.works.at = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి పని చేసే చోటు ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి పని చేసే చోటు ({ $day })
}

journal.window.entities.connections.you.latest.works.with = { $moments_quantity ->
    [one] 1 క్షణం కలిసి · ఇటీవలి కలిసి పని ({ $day })
   *[other] { $moments } క్షణాలు కలిసి · ఇటీవలి కలిసి పని ({ $day })
}

journal.window.entities.copy.cards.empty = ఈ ఫాసెట్‌లో ఇంకా ఎంటిటీలు లేవు. కింద మీరు ఉంచుకోవాలనుకునేవాటిని స్టార్ చేయండి, లేదా మీరే ఒకటి చేర్చండి.

journal.window.entities.copy.conn.ambiguous = ఇది ఒకటి కంటే ఎక్కువ ఎంటిటీలతో సరిపోవచ్చు.

journal.window.entities.copy.conn.calendar.only = క్యాలెండర్ మాత్రమే

journal.window.entities.copy.conn.empty = ఇంకా కనెక్షన్‌లు నమోదు అవ్వలేదు. ఈ ఎంటిటీ మరెవరితోనైనా లేదా మరేదానితోనైనా కలిసి కనిపించినప్పుడు అవి కనిపిస్తాయి.

journal.window.entities.copy.conn.events.only = ఈవెంట్‌లు మాత్రమే

journal.window.entities.copy.conn.evidence.caption = దీని వెనుక ఉన్న క్షణాలు: ఆధారం, తీర్మానం కాదు

journal.window.entities.copy.conn.evidence.failed = ఈ క్షణాలను లోడ్ చేయలేకపోయింది. మళ్ళీ ప్రయత్నించండి.

journal.window.entities.copy.conn.evidence.more = { $slot2 } లో { $slot1 } చూపిస్తోంది. మరిన్ని చూపండి

journal.window.entities.copy.conn.evidence.view.entity = ఎంటిటీని చూడండి →

journal.window.entities.copy.conn.horizon = ఒకే చోట గడిపిన సమయం నుండి కనెక్షన్‌లు { $slot1 } వరకు మాత్రమే వెనక్కి వెళ్తాయి. అంతకు ముందు { $slot2 } రోజులు ఇంకా మీ జర్నల్‌లో ఉన్నాయి. ఆ కనెక్షన్‌లు ఇంకా బిల్డ్ అవ్వలేదు.

journal.window.entities.copy.conn.horizon.one = ఒకే చోట గడిపిన సమయం నుండి కనెక్షన్‌లు { $slot1 } వరకు మాత్రమే వెనక్కి వెళ్తాయి. అంతకు ముందు ఒక రోజు ఇంకా మీ జర్నల్‌లో ఉంది. ఆ కనెక్షన్‌లు ఇంకా బిల్డ్ అవ్వలేదు.

journal.window.entities.copy.conn.index.action = సిస్టమ్ స్థితి సరిచూడండి →

journal.window.entities.copy.conn.index.unavailable = కనెక్షన్‌లు ఇప్పుడు అందుబాటులో లేవు.

journal.window.entities.copy.conn.kind.chip.words.mentioned = ప్రస్తావనలు

journal.window.entities.copy.conn.kind.words.attended.with = ఈవెంట్‌లు

journal.window.entities.copy.conn.kind.words.co.present = ఒకే చోట

journal.window.entities.copy.conn.kind.words.created = సృష్టించిన

journal.window.entities.copy.conn.kind.words.family.of = కుటుంబం

journal.window.entities.copy.conn.kind.words.knows = తెలుసు

journal.window.entities.copy.conn.kind.words.mentioned = ప్రస్తావించిన

journal.window.entities.copy.conn.kind.words.messaged.with = సందేశం

journal.window.entities.copy.conn.kind.words.other = సంబంధిత

journal.window.entities.copy.conn.kind.words.party.of = పాలుదారు

journal.window.entities.copy.conn.kind.words.reports.to = నివేదిస్తుంది

journal.window.entities.copy.conn.kind.words.scheduled.with = షెడ్యూల్

journal.window.entities.copy.conn.kind.words.spoke.with = మాట్లాడిన

journal.window.entities.copy.conn.kind.words.uses = వాడుతుంది

journal.window.entities.copy.conn.kind.words.works.at = పని చేసే చోటు

journal.window.entities.copy.conn.kind.words.works.with = కలిసి పని

journal.window.entities.copy.conn.label.fallbacks.co.present = ఒకే చోట గడిపిన సమయం

journal.window.entities.copy.conn.label.fallbacks.mentioned = సంభాషణలో వచ్చింది

journal.window.entities.copy.conn.label.fallbacks.spoke.with = ఒక సంభాషణ

journal.window.entities.copy.conn.load.failed = కనెక్షన్‌లను లోడ్ చేయలేకపోయింది. మళ్ళీ ప్రయత్నించండి.

journal.window.entities.copy.conn.row.meta = { $slot1 } క్షణాలు · { $slot2 }

journal.window.entities.copy.conn.row.meta.attendance = { $slot1 } ఈవెంట్‌లు కలిసి · { $slot2 }

journal.window.entities.copy.conn.row.meta.one = 1 క్షణం · { $slot1 }

journal.window.entities.copy.conn.row.meta.one.attendance = 1 ఈవెంట్ కలిసి · { $slot1 }

journal.window.entities.copy.conn.row.meta.scheduled = { $slot1 }

journal.window.entities.copy.conn.source.word.fallback = మీ జర్నల్ ఉంచుకున్నది

journal.window.entities.copy.conn.source.words.calendar = క్యాలెండర్ ఈవెంట్‌లు

journal.window.entities.copy.conn.source.words.co.presence = ఉమ్మడి క్షణాలు

journal.window.entities.copy.conn.source.words.document = డాక్యుమెంట్‌లు

journal.window.entities.copy.conn.source.words.event.legacy = పాత క్యాలెండర్ ఈవెంట్‌లు

journal.window.entities.copy.conn.source.words.mention = సంభాషణ

journal.window.entities.copy.conn.source.words.messaging = సందేశాలు

journal.window.entities.copy.conn.source.words.observation = మీ జర్నల్ ఉంచుకున్నది

journal.window.entities.copy.conn.source.words.participation = కార్యకలాప నమోదులు

journal.window.entities.copy.conn.source.words.relation = మీ జర్నల్ ఉంచుకున్నది

journal.window.entities.copy.conn.source.words.speaker = సంభాషణ

journal.window.entities.copy.conn.summary = { $slot1 } నేరుగా ఆధారంతో · { $slot2 } ఉమ్మడి ఈవెంట్‌ల నుండి మాత్రమే

journal.window.entities.copy.conn.summary.calendar = { $slot1 } మీ క్యాలెండర్ నుండి మాత్రమే

journal.window.entities.copy.conn.with.you.eyebrow = మీతో

journal.window.entities.copy.conn.with.you.failed = ఈ ఎంటిటీతో మీ సొంత చరిత్రను ఇప్పుడే సరిచూడలేకపోయింది.

journal.window.entities.copy.conn.with.you.summary = { $slot1 } క్షణాలు కలిసి · ఇటీవలి { $slot2 } ({ $slot3 })

journal.window.entities.copy.conn.with.you.summary.one = 1 క్షణం కలిసి · ఇటీవలి { $slot1 } ({ $slot2 })

journal.window.entities.copy.conn.with.you.summary.scheduled = మీ క్యాలెండర్‌లో మాత్రమే · ఇటీవలి { $slot1 }

journal.window.entities.copy.conn.with.you.toggle = దీని వెనుక ఉన్న క్షణాలు ›

journal.window.entities.copy.detach.confirm = { $slot2 } నుండి { $slot1 } విడదీయాలా? అది మీ జర్నల్‌లో ఉంటుంది, మీరు ఎప్పుడైనా మళ్ళీ జోడించవచ్చు.

journal.window.entities.copy.detach.done = { $slot1 } నుండి విడదీయబడింది. { $slot2 } ఇంకా మీ జర్నల్‌లో ఉంది.

journal.window.entities.copy.detach.find.action = మీ జర్నల్‌లో దాన్ని కనుక్కోండి →

journal.window.entities.copy.detach.reattach.action = దాన్ని మళ్ళీ జోడించండి →

journal.window.entities.copy.detected.all.hidden = ఇటీవల ఇక్కడ గుర్తించినదంతా దాచబడింది.

journal.window.entities.copy.detected.empty = ఇక్కడ ఇంకా ఏమీ లేదు. మీరు మీ జర్నల్‌కు చేర్చేకొద్దీ వ్యక్తులు, ప్రదేశాలు, వస్తువులు ఇక్కడ కనిపిస్తాయి.

journal.window.entities.copy.detected.hidden.badge = దాచబడింది

journal.window.entities.copy.detected.hidden.count = { $slot2 } లో { $slot1 } పేర్లు దాచబడ్డాయి

journal.window.entities.copy.detected.hidden.count.one = { $slot1 } లో 1 పేరు దాచబడింది

journal.window.entities.copy.detected.hidden.heading = { $slot1 } లో దాచబడింది

journal.window.entities.copy.detected.hidden.last = చివరిసారి గుర్తించినది { $slot1 }

journal.window.entities.copy.detected.hidden.lede = దాచిన పేరు మీ జర్నల్‌లో ఉంటుంది, మళ్ళీ ఎన్నిసార్లు గుర్తించినా దాచబడి ఉంటుంది. ఇక్కడ దాన్ని గుర్తించినవి వెతకడం నుండి మినహాయించబడ్డాయి, కనెక్ట్ అయిన ఏజెంట్‌లు వాటితో కనెక్ట్ కాలేవు, మీ జర్నల్ దీన్ని ఈ ఫాసెట్‌కు చేర్చదు.

journal.window.entities.copy.detected.hidden.show = దాచబడినవి చూపండి

journal.window.entities.copy.detected.hidden.stop.show = దాచబడినవి చూపడం ఆపండి

journal.window.entities.copy.detected.hide.action = { $slot2 } లో { $slot1 } దాచండి

journal.window.entities.copy.detected.hide.done = { $slot1 } { $slot2 } లో దాచబడింది.

journal.window.entities.copy.detected.hide.done.detail = మళ్ళీ ఎన్నిసార్లు గుర్తించినా అది దాచబడి ఉంటుంది.

journal.window.entities.copy.detected.hide.failed = { $slot1 } దాచలేకపోయింది. ఏమీ మారలేదు.

journal.window.entities.copy.detected.hide.no.search = వెతకడం మరియు కనెక్ట్ అయిన ఏజెంట్‌లు ఇక్కడ దాన్ని గుర్తించినవాటిని ఇంకా కనుక్కోవచ్చు.

journal.window.entities.copy.detected.hide.try.again = మళ్ళీ ప్రయత్నించండి

journal.window.entities.copy.detected.still.hidden = { $slot1 } ఇంకా { $slot2 } తో సరిపోతోంది, ఒక దాచబడిన ఎంటిటీ.

journal.window.entities.copy.detected.still.hidden.open = { $slot1 } తెరవండి

journal.window.entities.copy.detected.unhide = మళ్ళీ చూపండి

journal.window.entities.copy.detected.unhide.failed = { $slot1 } మళ్ళీ చూపలేకపోయింది. ఏమీ మారలేదు.

journal.window.entities.copy.detected.unreadable = ఇక్కడ గుర్తించినవి చూపబడలేవు. ఈ ఫాసెట్‌లో మీరు ఏ పేర్లు దాచారో మీ జర్నల్ చదవలేకపోయింది.

journal.window.entities.copy.faces.busy = మీ జర్నల్ ఇప్పుడు ముఖ డేటాతో బిజీగా ఉంది, కాబట్టి ఏమీ మారలేదు.

journal.window.entities.copy.faces.change = ప్రొఫైల్ చిత్రం మార్చండి

journal.window.entities.copy.faces.changed = పేజీ లోడ్ అయిన తర్వాత ఇది మారింది, కాబట్టి అన్‌డూ కాలేదు.

journal.window.entities.copy.faces.chosen.alt = ప్రొఫైల్ చిత్రం: { $slot1 }

journal.window.entities.copy.faces.confirmed.lede = మీరు { $slot1 } కోసం ఈ ప్రొఫైల్ చిత్రం ఎంచుకున్నారు.

journal.window.entities.copy.faces.disabled = ప్రొఫైల్ చిత్రం సూచనలు ఆఫ్ అయి ఉన్నాయి, కాబట్టి ఏమీ మారలేదు.

journal.window.entities.copy.faces.dismissed.lede = మీరు { $slot1 } కోసం ప్రొఫైల్ చిత్రం లేకుండా ఎంచుకున్నారు. { $slot2 } కావచ్చు అనే ఇతర ముఖాలు కనిపిస్తే, అవి ఇక్కడ కనిపించవచ్చు.

journal.window.entities.copy.faces.done.chosen = { $slot1 } కోసం ప్రొఫైల్ చిత్రం సేవ్ అయింది.

journal.window.entities.copy.faces.done.dismissed = { $slot1 } కోసం ప్రొఫైల్ చిత్రం లేదు.

journal.window.entities.copy.faces.done.rejected = సరే, అది { $slot1 } కాదు.

journal.window.entities.copy.faces.face.alt = { $slot1 } నాటి సమావేశం నుండి ఒక ముఖం, { $slot2 } కావచ్చు

journal.window.entities.copy.faces.facet.hint = { $slot1 } కోసం ప్రొఫైల్ చిత్రం ఎంచుకోండి

journal.window.entities.copy.faces.failed = అది సేవ్ కాలేదు.

journal.window.entities.copy.faces.from.day = { $slot1 } నాటి సమావేశం నుండి

journal.window.entities.copy.faces.gone = ఆ ముఖం ఇక అందుబాటులో లేదు.

journal.window.entities.copy.faces.heading = ప్రొఫైల్ చిత్రం

journal.window.entities.copy.faces.load.failed = ఈ ముఖాలను లోడ్ చేయలేకపోయింది.

journal.window.entities.copy.faces.merge.reason = మీ సమావేశాల నుండి ముఖాలు ఇవి ఒకే వ్యక్తి కావచ్చు అని సూచిస్తున్నాయి.

journal.window.entities.copy.faces.none = వీటిలో ఏదీ కాదు

journal.window.entities.copy.faces.not.them = { $slot1 } కాదు

journal.window.entities.copy.faces.possible.lede = మీ సమావేశాల నుండి ఈ ముఖాలు { $slot1 } కావచ్చు. మీరు ఒకటి ఎంచుకునే వరకు ఏదీ వాడబడదు.

journal.window.entities.copy.faces.possible.title = { $slot1 } కోసం ప్రొఫైల్ చిత్రం ఎంచుకోండి

journal.window.entities.copy.faces.refresh = రిఫ్రెష్ చేయండి

journal.window.entities.copy.faces.remove = ప్రొఫైల్ చిత్రం వాడకండి

journal.window.entities.copy.faces.retry = మళ్ళీ ప్రయత్నించండి

journal.window.entities.copy.faces.see.others = వేరొకటి ఎంచుకోండి

journal.window.entities.copy.faces.settings.link = సెట్టింగ్‌లలో ఆన్ చేయండి

journal.window.entities.copy.faces.show.again = అన్‌డూ చేయండి

journal.window.entities.copy.faces.source.anchored = పేరు పక్కన

journal.window.entities.copy.faces.source.box = వీడియో టైల్‌లో

journal.window.entities.copy.faces.stale = పేజీ లోడ్ అయిన తర్వాత ఇది మారింది, కాబట్టి ఏమీ సేవ్ కాలేదు.

journal.window.entities.copy.faces.suggested.badge = సూచన

journal.window.entities.copy.faces.suggested.lede = మీ జర్నల్ ఈ ముఖాన్ని మీ సమావేశ రికార్డింగ్‌ల నుండి ఎంచుకుంది. మీరు అవును అనే వరకు ఇది సూచన మాత్రమే.

journal.window.entities.copy.faces.suggested.title = ఇది { $slot1 } అవునా?

journal.window.entities.copy.faces.unconfirmed.alt = ఒక సమావేశం నుండి ఒక ముఖం, { $slot1 } కావచ్చు, నిర్ధారణ కాలేదు

journal.window.entities.copy.faces.unconfirmed.badge = నిర్ధారణ కాలేదు

journal.window.entities.copy.faces.undo = అన్‌డూ చేయండి

journal.window.entities.copy.faces.undone = అన్‌డూ అయింది.

journal.window.entities.copy.faces.use = ఈ ప్రొఫైల్ చిత్రం వాడండి

journal.window.entities.copy.faces.yes = అవును, ఈ ప్రొఫైల్ చిత్రం వాడండి

journal.window.entities.copy.grid.clear.range = కాల పరిధి క్లియర్ చేయండి

journal.window.entities.copy.grid.day.one = రోజు

journal.window.entities.copy.grid.day.other = రోజులు

journal.window.entities.copy.grid.default.day = ఇటీవలి రోజు: { $slot1 }

journal.window.entities.copy.grid.lede = { $slot3 } { $slot4 } అంతటా { $slot1 } { $slot2 }

journal.window.entities.copy.grid.range = { $slot1 } – { $slot2 }

journal.window.entities.copy.grid.show.all = అన్ని { $slot1 } { $slot2 } చూపండి

journal.window.entities.copy.grid.title = కాలక్రమంలో క్షణాలు

journal.window.entities.copy.grid.unit.none = ఇంకా ఏమీ లేదు

journal.window.entities.copy.grid.unit.one = క్షణం

journal.window.entities.copy.grid.unit.other = క్షణాలు

journal.window.entities.copy.hide.pane.help = దాన్ని దాచబడినవాటిలోకి తరలిస్తుంది. అక్కడ మీరు దాన్ని మళ్ళీ చూపవచ్చు. దాని నోట్‌లు, దాని ఫాసెట్‌లలో దాన్ని గుర్తించినవి వెతకడం నుండి బయటకు వెళ్తాయి, కనెక్ట్ అయిన ఏజెంట్‌లు వాటితో కనెక్ట్ కాలేవు.

journal.window.entities.copy.obs.disclosure = { $slot2 } క్షణాలలో ఇటీవలి { $slot1 } చూపిస్తోంది

journal.window.entities.copy.obs.empty = { $slot1 } తో ఇంకా ఏమీ లేదు.

journal.window.entities.copy.obs.heading = క్షణాలు

journal.window.entities.copy.obs.load.failed = ఈ క్షణాలను లోడ్ చేయలేకపోయింది. మళ్ళీ ప్రయత్నించండి →

journal.window.entities.copy.obs.source.link.title = ఈ రోజు చూడండి

journal.window.entities.copy.scope.empty.action = మీ జర్నల్ మొత్తం చూపండి

journal.window.entities.copy.scope.empty.body = ఈ ఫాసెట్‌లో వ్యక్తులను, ప్రదేశాలను, ప్రాజెక్ట్‌లను మీ జర్నల్ ఉంచుకున్న కొద్దీ ఎంటిటీలు ఇక్కడ కనిపిస్తాయి.

journal.window.entities.copy.scope.empty.title = { $slot1 } లో ఇంకా ఏమీ లేదు.

journal.window.entities.copy.scope.facet.missing = ఆ ఫాసెట్ ఇక మీ జర్నల్‌లో లేదు, కాబట్టి ఇది మీ జర్నల్ మొత్తం.

journal.window.entities.copy.scope.showing = చూపిస్తోంది

journal.window.entities.copy.scope.whole.journal = మీ జర్నల్ మొత్తం

journal.window.entities.copy.trust.action.failed = ఆ ఎంటిటీ మార్పు పూర్తి కాలేదు.

journal.window.entities.copy.trust.heading = గుర్తింపు నమ్మకం

journal.window.entities.copy.trust.history.change = { $slot1 } → { $slot2 }

journal.window.entities.copy.trust.history.empty = ఇంకా గుర్తింపు చరిత్ర అందుబాటులో లేదు.

journal.window.entities.copy.trust.history.heading = వెర్షన్ చరిత్ర

journal.window.entities.copy.trust.history.suggested.by = { $slot1 } ద్వారా సూచన

journal.window.entities.copy.trust.history.summary = { $slot1 } · { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.identity.none = ముందటి గుర్తింపు లేదు

journal.window.entities.copy.trust.identity.summary = { $slot1 } · { $slot2 } · ఇలా కూడా: { $slot3 }

journal.window.entities.copy.trust.load.failed = గుర్తింపు నమ్మకం వివరాలను లోడ్ చేయలేకపోయింది. మళ్ళీ ప్రయత్నించండి.

journal.window.entities.copy.trust.merge.action = జతను ప్రివ్యూ చూడండి

journal.window.entities.copy.trust.merge.confirm = { $slot2 } లోకి { $slot1 } జత చేయాలా? జతను అన్‌డూ చేయడం సాధ్యం కాదు.

journal.window.entities.copy.trust.merge.confirm.action = జతను నిర్ధారించండి

journal.window.entities.copy.trust.merge.done = జత అయింది.

journal.window.entities.copy.trust.merge.label = లోకి జత చేయండి

journal.window.entities.copy.trust.preview.counts = చేర్చిన గొంతు నమూనాలు: { $slot1 } · జత తర్వాత నమూనాలు: { $slot2 } · జత అవుతున్న పేరు కింద నోట్‌లు: { $slot3 } · మారిన మాట్లాడిన వారి లేబుల్‌లు: { $slot4 } · మారిన కార్యకలాప నమోదులు: { $slot5 }

journal.window.entities.copy.trust.preview.summary = చేర్చిన అలియాస్‌లు: { $slot1 } · చేర్చిన ఈమెయిల్‌లు: { $slot2 }

journal.window.entities.copy.trust.restore.action = ఈ వెర్షన్‌ను రీస్టోర్ చేయండి

journal.window.entities.copy.trust.restore.confirm = ఈ వెర్షన్ నుండి గుర్తింపును రీస్టోర్ చేయాలా? ప్రస్తుత గుర్తింపు చరిత్రలో ఉంటుంది.

journal.window.entities.copy.trust.suggestion.summary = { $slot1 } → { $slot2 } · { $slot3 }

journal.window.entities.copy.trust.suggestions.empty = ఈ ఎంటిటీకి సంబంధించిన తెరిచి ఉన్న జత సూచనలు లేవు.

journal.window.entities.copy.trust.suggestions.heading = తెరిచి ఉన్న జత సూచనలు

journal.window.entities.copy.voice.changed = ఆ నమూనా మారింది. దాన్ని తీసివేయడానికి ముందు జాబితాను రిఫ్రెష్ చేయండి.

journal.window.entities.copy.voice.empty = ఈ వ్యక్తి కోసం గొంతు నమూనాలు సేవ్ అయి లేవు.

journal.window.entities.copy.voice.error = ఆ గొంతు నమూనాలను చదవలేకపోయింది లేదా మార్చలేకపోయింది. మళ్ళీ ప్రయత్నించండి.

journal.window.entities.copy.voice.heading = గొంతు నమూనాలు

journal.window.entities.copy.voice.loading = గొంతు నమూనాలు లోడ్ అవుతున్నాయి…

journal.window.entities.copy.voice.more = మరిన్ని చూపండి

journal.window.entities.copy.voice.open = రికార్డింగ్ తెరవండి

journal.window.entities.copy.voice.remove = తీసివేయండి

journal.window.entities.copy.voice.retry = మళ్ళీ ప్రయత్నించండి

journal.window.entities.copy.voice.sentence = వాక్యం { $slot1 }

journal.window.entities.delete.all.items.gone = { $removed_quantity ->
    [one] ఇది ఇక ఇక్కడ జాబితాలో లేదు.
   *[other] ఇవి ఇక ఇక్కడ జాబితాలో లేవు.
}

journal.window.entities.delete.couldn.t.erase.that = ఆ ఎంటిటీని తుడిచివేయలేకపోయింది

journal.window.entities.delete.couldn.t.recover.that = ఆ ఎంటిటీని రీస్టోర్ చేయలేకపోయింది

journal.window.entities.delete.detail.action.help = { $action_start }తొలగించండి{ $action_end } { $description_start }దాన్ని ట్రాష్‌కు తరలిస్తుంది, తుడిచివేయబడే వరకు అక్కడ మీరు రీస్టోర్ చేయవచ్చు.{ $description_end }

journal.window.entities.delete.erased.from.your.journal = { $total_quantity ->
    [one] మీ జర్నల్ నుండి { $total } ఎంటిటీని తుడిచివేసింది
   *[other] మీ జర్నల్ నుండి { $total } ఎంటిటీలను తుడిచివేసింది
}

journal.window.entities.delete.erased.of = { $processed_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $processed } తుడిచివేయబడింది
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $processed } తుడిచివేయబడింది
}

journal.window.entities.delete.erases.unless.you.recover.it = మీరు దాన్ని రీస్టోర్ చేయకపోతే { $erase_day } తుడిచివేస్తుంది.

journal.window.entities.delete.facet.many.its.place.and.notes.in = { $facets_quantity ->
    [one] { $facets } ఫాసెట్‌లో దాని స్థానం, నోట్‌లు
   *[other] { $facets } ఫాసెట్‌లలో దాని స్థానం, నోట్‌లు
}

journal.window.entities.delete.gone.more.many.more.are.no.longer.listed = { $gone_quantity ->
    [one] మరో { $gone } ఇక జాబితాలో లేదు.
   *[other] మరిన్ని { $gone } ఇక జాబితాలో లేవు.
}

journal.window.entities.delete.items.find.after.failed.erase = { $remaining_quantity ->
    [one] కింద లేదా ట్రాష్‌లో దాన్ని వెతకండి.
   *[other] కింద లేదా ట్రాష్‌లో వాటిని వెతకండి.
}

journal.window.entities.delete.many.delete.entities = { $total_quantity ->
    [one] { $total } ఎంటిటీని తొలగించాలా?
   *[other] { $total } ఎంటిటీలను తొలగించాలా?
}

journal.window.entities.delete.many.erase.entities.now = { $total_quantity ->
    [one] { $total } ఎంటిటీని ఇప్పుడు తుడిచివేయాలా?
   *[other] { $total } ఎంటిటీలను ఇప్పుడు తుడిచివేయాలా?
}

journal.window.entities.delete.marked.many.couldn.t.be.moved.and = { $remaining_quantity ->
    [one] { $remaining } తరలించలేకపోయింది, కింద గుర్తు ఉంది.
   *[other] { $remaining } తరలించలేకపోయాయి, కింద గుర్తు ఉంది.
}

journal.window.entities.delete.more.and.more = { $more_quantity ->
    [one] మరియు మరో { $more }
   *[other] మరియు మరో { $more }
}

journal.window.entities.delete.moved.of.to.the = { $processed_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $processed } ట్రాష్‌కు తరలించబడింది
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $processed } ట్రాష్‌కు తరలించబడింది
}

journal.window.entities.delete.moved.to.the.trash = { $total_quantity ->
    [one] { $total } ఎంటిటీని ట్రాష్‌కు తరలించింది
   *[other] { $total } ఎంటిటీలను ట్రాష్‌కు తరలించింది
}

journal.window.entities.delete.none.still.many.are.still.in.your.journal = { $remaining_quantity ->
    [one] { $remaining } ఇంకా మీ జర్నల్‌లో ఉంది, కింద గుర్తు ఉంది.
   *[other] { $remaining } ఇంకా మీ జర్నల్‌లో ఉన్నాయి, కింద గుర్తు ఉంది.
}

journal.window.entities.delete.noun.erased.from.your.journal = మీ జర్నల్ నుండి ఎంటిటీ తుడిచివేయబడింది

journal.window.entities.delete.other.items.gone = { $other_quantity ->
    [one] మరొకటి ఇక ఇక్కడ జాబితాలో లేదు.
   *[other] మిగతావి ఇక ఇక్కడ జాబితాలో లేవు.
}

journal.window.entities.delete.partial.items.not.erased = { $remaining_quantity ->
    [one] మరొకటి తుడిచివేయబడలేదు. కింద లేదా ట్రాష్‌లో దాన్ని వెతకండి.
   *[other] మిగతావి తుడిచివేయబడలేదు. కింద లేదా ట్రాష్‌లో వాటిని వెతకండి.
}

journal.window.entities.delete.partial.items.not.recovered = { $remaining_quantity ->
    [one] మరొకటి ఇంకా ట్రాష్‌లో ఉంది. అక్కడి నుండి మీరు మళ్ళీ ప్రయత్నించవచ్చు.
   *[other] మిగతావి ఇంకా ట్రాష్‌లో ఉన్నాయి. అక్కడి నుండి మీరు మళ్ళీ ప్రయత్నించవచ్చు.
}

journal.window.entities.delete.pick.label.select.name = { $entity_name } ఎంచుకోండి

journal.window.entities.delete.recovered.count = { $total_quantity ->
    [one] { $total } ఎంటిటీ రీస్టోర్ అయింది
   *[other] { $total } ఎంటిటీలు రీస్టోర్ అయ్యాయి
}

journal.window.entities.delete.recovered.of = { $processed_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $processed } రీస్టోర్ అయింది
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $processed } రీస్టోర్ అయింది
}

journal.window.entities.delete.recovered.one = ఎంటిటీ రీస్టోర్ అయింది

journal.window.entities.delete.selected.selected = { $selected_quantity ->
    [one] { $selected } ఎంచుకోబడింది
   *[other] { $selected } ఎంచుకోబడింది
}

journal.window.entities.delete.still.many.couldn.t.be.moved.and = { $remaining_quantity ->
    [one] { $remaining } తరలించలేకపోయింది, ఇంకా మీ జర్నల్‌లో ఉంది, కింద గుర్తు ఉంది.
   *[other] { $remaining } తరలించలేకపోయాయి, ఇంకా మీ జర్నల్‌లో ఉన్నాయి, కింద గుర్తు ఉంది.
}

journal.window.entities.delete.they.erase.unless.you.recover = మీరు వాటిని రీస్టోర్ చేయకపోతే అవి { $erase_day } తుడిచివేస్తాయి.

journal.window.entities.detail.aliases.other.names = ఇలా కూడా: { $aliases }

journal.window.entities.detail.facets.count = { $facets_quantity ->
    [one] 1 ఫాసెట్
   *[other] { $facets } ఫాసెట్‌లు
}

journal.window.entities.detail.id.description = ఈ పేరు ID లాగా కనిపిస్తోంది.

journal.window.entities.detail.last.mentioned.date = చివరిసారి ప్రస్తావన: { $day }

journal.window.entities.detail.last.mentioned.full.day = చివరిసారి ప్రస్తావన: { $day }

journal.window.entities.detected.hidden.heading = { $facet_name } లో దాచబడింది

journal.window.entities.detected.hidden.in.facet = { $hidden_quantity ->
    [one] { $facet_name } లో 1 పేరు దాచబడింది
   *[other] { $facet_name } లో { $hidden } పేర్లు దాచబడ్డాయి
}

journal.window.entities.detected.hidden.last.detected = చివరిసారి గుర్తించడం { $day }

journal.window.entities.detected.hidden.reappeared = { $entity_name } ఇంకా { $matched_name } తో సరిపోతోంది, అది దాచబడిన ఎంటిటీ.

journal.window.entities.detected.hidden.reappeared.open = { $matched_name } తెరవండి

journal.window.entities.detected.hide.action = { $facet_name } లో { $entity_name } దాచండి

journal.window.entities.detected.hide.done = { $entity_name } { $facet_name } లో దాచబడింది.

journal.window.entities.detected.hide.failed = { $entity_name } దాచలేకపోయింది. ఏమీ మారలేదు.

journal.window.entities.detected.page.range = { $total_quantity ->
    [one] { $total } లో { $first }–{ $last } గుర్తించిన ఎంటిటీ చూపిస్తోంది
   *[other] { $total } లో { $first }–{ $last } గుర్తించిన ఎంటిటీలు చూపిస్తోంది
}

journal.window.entities.detected.unhide.failed = { $entity_name } మళ్ళీ చూపలేకపోయింది. ఏమీ మారలేదు.

journal.window.entities.entity.delete.copy.again.if.your.journal.finds.it.again = మీ జర్నల్ దీన్ని తర్వాత మళ్ళీ కనుక్కుంటే, అది కొత్త ఎంటిటీగా చేర్చబడుతుంది.

journal.window.entities.entity.delete.copy.again.many.any.of.them.your.journal.finds = వాటిలో మీ జర్నల్ తర్వాత మళ్ళీ కనుక్కునేది కొత్త ఎంటిటీగా చేర్చబడుతుంది.

journal.window.entities.entity.delete.copy.back.back = వెనుకకు

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.it.right = మరేదో దీన్ని ఇప్పుడు మారుస్తోంది. ఒక నిమిషం తర్వాత మళ్ళీ ప్రయత్నించండి.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right = మరేదో దీన్ని ఇప్పుడు మారుస్తోంది, కాబట్టి ఇది తరలించబడలేదు. ఒక నిమిషం తర్వాత మళ్ళీ ప్రయత్నించండి.

journal.window.entities.entity.delete.copy.busy.something.else.is.changing.this.right.variant2 = మరేదో దీన్ని ఇప్పుడు మారుస్తోంది, కాబట్టి ఇది తుడిచివేయబడలేదు. ఒక నిమిషం తర్వాత మళ్ళీ ప్రయత్నించండి.

journal.window.entities.entity.delete.copy.button.delete = తొలగించండి

journal.window.entities.entity.delete.copy.cancel.cancel = రద్దు చేయండి

journal.window.entities.entity.delete.copy.delete.delete = తొలగించండి

journal.window.entities.entity.delete.copy.details.its.details.and.version.history = దాని వివరాలు, వెర్షన్ చరిత్ర

journal.window.entities.entity.delete.copy.done.done = పూర్తి

journal.window.entities.entity.delete.copy.erase.now.erase.now = ఇప్పుడు తుడిచివేయండి

journal.window.entities.entity.delete.copy.facet.one.its.place.and.notes.in.facet = 1 ఫాసెట్‌లో దాని స్థానం, నోట్‌లు

journal.window.entities.entity.delete.copy.facet.unknown.its.place.and.notes.in.each = అది ఉన్న ప్రతి ఫాసెట్‌లో దాని స్థానం, నోట్‌లు

journal.window.entities.entity.delete.copy.gone.it.s.no.longer.in.your = ఇది ఇక మీ జర్నల్‌లో లేదు.

journal.window.entities.entity.delete.copy.last.facet.your.journal.needs.at.least.one = మ్యూట్ కాని ఫాసెట్ ఒకటైనా మీ జర్నల్‌కు కావాలి.

journal.window.entities.entity.delete.copy.lead.this.erases.the.entity.s.own = ఇది ఎంటిటీ సొంతవాటిని తుడిచివేస్తుంది:

journal.window.entities.entity.delete.copy.lead.this.moves.the.entity.s.own = ఇది ఎంటిటీ సొంతవాటిని తరలిస్తుంది:

journal.window.entities.entity.delete.copy.move.to.trash.move.to.trash = ట్రాష్‌కు తరలించండి

journal.window.entities.entity.delete.copy.open.trash.open.trash = ట్రాష్ తెరవండి

journal.window.entities.entity.delete.copy.other.this.wasn.t.erased.try.again = ఇది తుడిచివేయబడలేదు. మళ్ళీ ప్రయత్నించండి.

journal.window.entities.entity.delete.copy.other.this.wasn.t.moved.try.again = ఇది తరలించబడలేదు. మళ్ళీ ప్రయత్నించండి.

journal.window.entities.entity.delete.copy.own.many.this.erases.each.one.s.own = ఇది ప్రతిదాని సొంత వివరాలను, వెర్షన్ చరిత్రను, ప్రతి ఫాసెట్‌లోని స్థానాన్ని, నోట్‌లను, దాని గొంతు గురించి మీ జర్నల్ ఉంచుకున్నదాన్ని, ఏదైనా ఉంటే, తుడిచివేస్తుంది.

journal.window.entities.entity.delete.copy.own.many.this.moves.each.one.s.own = ఇది ప్రతిదాని సొంత వివరాలను, వెర్షన్ చరిత్రను, ప్రతి ఫాసెట్‌లోని స్థానాన్ని, నోట్‌లను, దాని గొంతు గురించి మీ జర్నల్ ఉంచుకున్నదాన్ని, ఏదైనా ఉంటే, తరలిస్తుంది.

journal.window.entities.entity.delete.copy.principal.this.is.you.so.it.can = ఇది మీరు, కాబట్టి దీన్ని తొలగించలేం.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and = ఇది ఇప్పుడు మీ జర్నల్ నుండి వెళ్తుంది, రీస్టోర్ చేయలేరు.

journal.window.entities.entity.delete.copy.reach.it.leaves.your.journal.now.and.variant2 = ఇది ఇప్పుడు మీ జర్నల్ నుండి వెళ్లి ట్రాష్‌లో వేచి ఉంటుంది, తర్వాత తుడిచివేయబడుతుంది. అప్పటి వరకు రీస్టోర్ చేయవచ్చు.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and = ఇవి ఇప్పుడు మీ జర్నల్ నుండి వెళ్తాయి, రీస్టోర్ చేయలేరు.

journal.window.entities.entity.delete.copy.reach.many.they.leave.your.journal.now.and.variant2 = ఇవి ఇప్పుడు మీ జర్నల్ నుండి వెళ్లి ట్రాష్‌లో వేచి ఉంటాయి, తర్వాత తుడిచివేయబడతాయి. అప్పటి వరకు ప్రతిదాన్ని రీస్టోర్ చేయవచ్చు.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its = ఎంటిటీల నమోదును మీ జర్నల్ అప్‌డేట్ చేయలేకపోయింది. ఏమీ తరలలేదు.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant2 = ఎంటిటీల నమోదును మీ జర్నల్ అప్‌డేట్ చేయలేకపోయింది. ఏమీ తుడిచివేయబడలేదు.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant3 = ఎంటిటీల నమోదును మీ జర్నల్ అప్‌డేట్ చేయలేకపోయింది, కాబట్టి ఇది తరలించబడలేదు.

journal.window.entities.entity.delete.copy.record.damaged.your.journal.couldn.t.update.its.variant4 = ఎంటిటీల నమోదును మీ జర్నల్ అప్‌డేట్ చేయలేకపోయింది, కాబట్టి ఇది తుడిచివేయబడలేదు.

journal.window.entities.entity.delete.copy.recover.recover = రీస్టోర్ చేయండి

journal.window.entities.entity.delete.copy.select.all.page.select.all.on.this.page = ఈ పేజీలో అన్నీ ఎంచుకోండి

journal.window.entities.entity.delete.copy.select.all.select.all = అన్నీ ఎంచుకోండి

journal.window.entities.entity.delete.copy.select.select = ఎంచుకోండి

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in = మీ జర్నల్‌లో మరెక్కడో ఉన్న ట్రాన్స్క్రిప్ట్‌లు, సారాంశాలు, కార్యకలాపాలు తుడిచివేయబడవు, వాటిని ఇంకా ప్రస్తావించవచ్చు.

journal.window.entities.entity.delete.copy.stays.many.transcripts.summaries.and.activities.elsewhere.in.variant2 = మీ జర్నల్‌లో మరెక్కడో ఉన్న ట్రాన్స్క్రిప్ట్‌లు, సారాంశాలు, కార్యకలాపాలు అక్కడే ఉంటాయి, వాటిని ఇంకా ప్రస్తావించవచ్చు.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in = మీ జర్నల్‌లో మరెక్కడో ఉన్న ట్రాన్స్క్రిప్ట్‌లు, సారాంశాలు, కార్యకలాపాలు తుడిచివేయబడవు, దీన్ని ఇంకా ప్రస్తావించవచ్చు.

journal.window.entities.entity.delete.copy.stays.transcripts.summaries.and.activities.elsewhere.in.variant2 = మీ జర్నల్‌లో మరెక్కడో ఉన్న ట్రాన్స్క్రిప్ట్‌లు, సారాంశాలు, కార్యకలాపాలు అక్కడే ఉంటాయి, దీన్ని ఇంకా ప్రస్తావించవచ్చు.

journal.window.entities.entity.delete.copy.voice.unknown.what.your.journal.keeps.about.its = దాని గొంతు గురించి మీ జర్నల్ ఉంచుకున్నది, ఏదైనా ఉంటే

journal.window.entities.entity.delete.copy.voice.what.your.journal.keeps.about.its = దాని గొంతు గురించి మీ జర్నల్ ఉంచుకున్నది

journal.window.entities.examples.first.and.second = ఉదా. { $first_name } మరియు { $second_name }

journal.window.entities.examples.names = ఉదా. { $names }

journal.window.entities.faces.chosen.alt = { $entity_name } ప్రొఫైల్ చిత్రం

journal.window.entities.faces.confirmed.lede = మీరు { $entity_name } కోసం ఈ ప్రొఫైల్ చిత్రం ఎంచుకున్నారు.

journal.window.entities.faces.dismissed.lede = మీరు { $entity_name } కోసం ప్రొఫైల్ చిత్రం ఎంచుకోలేదు. { $entity_name } కావచ్చని అనిపించే ఇతర ముఖాలు కనబడితే, అవి ఇక్కడ కనిపించవచ్చు.

journal.window.entities.faces.done.chosen = { $entity_name } కోసం ప్రొఫైల్ చిత్రం సేవ్ అయింది.

journal.window.entities.faces.done.dismissed = { $entity_name } కోసం ప్రొఫైల్ చిత్రం లేదు.

journal.window.entities.faces.done.rejected = సరే, అది { $entity_name } కాదు.

journal.window.entities.faces.face.alt = { $day } నాటి సమావేశం నుండి ఒక ముఖం, అది { $entity_name } కావచ్చు

journal.window.entities.faces.facet.hint = { $entity_name } కోసం ప్రొఫైల్ చిత్రం ఎంచుకోండి

journal.window.entities.faces.from.day = { $day } నాటి సమావేశం నుండి

journal.window.entities.faces.not.them = { $entity_name } కాదు

journal.window.entities.faces.possible.lede = మీ సమావేశాల నుండి ఈ ముఖాలు { $entity_name } కావచ్చు. మీరు ఒకదాన్ని ఎంచుకునే వరకు ఏదీ వాడబడదు.

journal.window.entities.faces.possible.title = { $entity_name } కోసం ప్రొఫైల్ చిత్రం ఎంచుకోండి

journal.window.entities.faces.suggested.title = ఇది { $entity_name } అవునా?

journal.window.entities.faces.unconfirmed.alt = ఒక సమావేశం నుండి ఒక ముఖం, అది { $entity_name } కావచ్చు, నిర్ధారణ కాలేదు

journal.window.entities.facet.detach.confirm = { $facet_name } నుండి { $entity_name } విడదీయాలా? అది మీ జర్నల్‌లో ఉంటుంది, ఎప్పుడైనా మళ్ళీ జోడించవచ్చు.

journal.window.entities.facet.detach.done = { $facet_name } నుండి విడిపోయింది. { $entity_name } ఇంకా మీ జర్నల్‌లో ఉంది.

journal.window.entities.facet.empty.heading = { $facet_name } లో ఇంకా ఏమీ లేదు.

journal.window.entities.facet.membership.list = { $leading_facets } లో, { $final_facet } లో

journal.window.entities.facet.membership.single = { $facet_name } లో

journal.window.entities.field.label.alternative.names.akas = ప్రత్యామ్నాయ పేర్లు (ఇలా కూడా)

journal.window.entities.field.label.description = వివరణ

journal.window.entities.field.label.name = పేరు

journal.window.entities.field.label.type = రకం

journal.window.entities.field.placeholder.alias1.alias2.alias3 = అలియాస్1, అలియాస్2, అలియాస్3

journal.window.entities.field.placeholder.e.g.lead.engineer.on.the = ఉదా., ప్రాజెక్ట్‌లో ప్రధాన ఇంజనీర్, అమ్మకాలకు ప్రధాన సంప్రదింపు…

journal.window.entities.field.placeholder.enter.description = వివరణ ఇవ్వండి…

journal.window.entities.field.placeholder.search.entities = ఎంటిటీలు వెతకండి…

journal.window.entities.field.placeholder.type.a.name.to.find.it = కనుక్కోవడానికి పేరు టైప్ చేయండి

journal.window.entities.folder.all = అన్నీ

journal.window.entities.folder.companies = సంస్థలు

journal.window.entities.folder.duplicates = సాధ్యమైన నకలులు

journal.window.entities.folder.hidden = దాచబడింది

journal.window.entities.folder.other = ఇతరం

journal.window.entities.folder.people = వ్యక్తులు

journal.window.entities.folder.projects = ప్రాజెక్ట్‌లు

journal.window.entities.folder.tools = టూల్స్

journal.window.entities.heading.couldn.t.load.entities = ఎంటిటీలు లోడ్ చేయలేకపోయింది

journal.window.entities.heading.that.search.couldn.t.be.run = ఆ శోధన నడపలేకపోయింది

journal.window.entities.heading.this.entity.couldn.t.be.loaded = ఈ ఎంటిటీని లోడ్ చేయలేకపోయింది

journal.window.entities.hide.already.absent.count = { $existing_quantity ->
    [one] 1 ఇప్పటికే దాచబడింది లేదా పోయింది.
   *[other] { $existing } ఇప్పటికే దాచబడ్డాయి లేదా పోయాయి.
}

journal.window.entities.hide.detail.action.help = { $action_start }దాచండి{ $action_end } { $description_start }దీన్ని దాచబడినవాటి దగ్గరకు తరలిస్తుంది, అక్కడ దీన్ని మళ్ళీ చూపవచ్చు. దాని నోట్‌లు, దాని ఫాసెట్‌లలోని గుర్తించడాలు శోధన నుండి వెళ్తాయి, కనెక్ట్ అయిన ఏజెంట్‌లు వాటిని చేరలేవు.{ $description_end }

journal.window.entities.hide.done.count = { $hidden_quantity ->
    [one] 1 ఎంటిటీ దాచబడింది.
   *[other] { $hidden } ఎంటిటీలు దాచబడ్డాయి.
}

journal.window.entities.hide.partial = { $remaining_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $hidden_quantity ->
        [one] { $hidden }
       *[other] { $hidden }
    } దాచబడింది. మిగిలినది దాచబడలేదు.
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $hidden_quantity ->
        [one] { $hidden }
       *[other] { $hidden }
    } దాచబడింది. మిగిలినవి దాచబడలేదు.
}

journal.window.entities.hide.paused.records = { $records_quantity ->
    [one] దాచడం పాజ్ అయి ఉంది: మీ జర్నల్‌లోని ఒక నమోదును చదవలేకపోయింది ({ $paths }).
   *[other] దాచడం పాజ్ అయి ఉంది: మీ జర్నల్‌లోని { $records } నమోదులను చదవలేకపోయింది ({ $paths }).
}

journal.window.entities.hide.progress = { $done_quantity ->
    [one] దాస్తోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $done }
   *[other] దాస్తోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $done }
}

journal.window.entities.hide.record.error = వాటిని దాచలేకపోయింది: మీ జర్నల్‌లోని ఒక నమోదును చదవలేకపోయింది ({ $record_detail }).

journal.window.entities.hide.selection.count = { $selected_quantity ->
    [one] { $selected } దాచండి
   *[other] { $selected } దాచండి
}

journal.window.entities.hide.selection.progress = { $done_quantity ->
    [one] దాస్తోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $done }
   *[other] దాస్తోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $done }
}

journal.window.entities.hide.undo.done = { $restored_quantity ->
    [one] 1 ఎంటిటీ తిరిగి పెట్టబడింది.
   *[other] { $restored } ఎంటిటీలు తిరిగి పెట్టబడ్డాయి.
}

journal.window.entities.hide.undo.help = { $hidden_quantity ->
    [one] అన్‌డూ దీన్ని తిరిగి పెడుతుంది, ఫాసెట్‌లతో సహా. మీకు కావాల్సినప్పుడు అది దాచబడినవాటి కింద కూడా ఉంటుంది.
   *[other] అన్‌డూ వాటిని తిరిగి పెడుతుంది, ఫాసెట్‌లతో సహా. మీకు కావాల్సినప్పుడు అవి దాచబడినవాటి కింద కూడా ఉంటాయి.
}

journal.window.entities.hide.undo.partial = { $failed_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $restored_quantity ->
        [one] { $restored }
       *[other] { $restored }
    } తిరిగి పెట్టబడింది. 1 తిరిగి పెట్టలేకపోయింది; అది ఇంకా దాచబడినవాటి కింద ఉంది.
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $restored_quantity ->
        [one] { $restored }
       *[other] { $restored }
    } తిరిగి పెట్టబడింది. { $failed } తిరిగి పెట్టలేకపోయింది; అవి ఇంకా దాచబడినవాటి కింద ఉన్నాయి.
}

journal.window.entities.hide.undo.progress = { $restored_quantity ->
    [one] తిరిగి పెడుతోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $restored }
   *[other] తిరిగి పెడుతోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $restored }
}

journal.window.entities.hide.undo.records.missing.one.entity = { $records_quantity ->
    [one] అది ఫాసెట్‌లు అన్నీ లేకుండా తిరిగి వచ్చి ఉండవచ్చు: మీ జర్నల్‌లోని ఒక నమోదును చదవలేకపోయింది ({ $paths }).
   *[other] అది ఫాసెట్‌లు అన్నీ లేకుండా తిరిగి వచ్చి ఉండవచ్చు: మీ జర్నల్‌లోని { $records } నమోదులను చదవలేకపోయింది ({ $paths }).
}

journal.window.entities.hide.undo.records.missing.several.entities = { $records_quantity ->
    [one] ఫాసెట్‌లు అన్నీ లేకుండా కొన్ని తిరిగి వచ్చి ఉండవచ్చు: మీ జర్నల్‌లోని ఒక నమోదును చదవలేకపోయింది ({ $paths }).
   *[other] ఫాసెట్‌లు అన్నీ లేకుండా కొన్ని తిరిగి వచ్చి ఉండవచ్చు: మీ జర్నల్‌లోని { $records } నమోదులను చదవలేకపోయింది ({ $paths }).
}

journal.window.entities.hide.undo.stopped = { $restored_quantity ->
    [one] అన్‌డూ { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $restored } తిరిగి పెట్టాక ఆగింది. మిగిలినవి ఇంకా దాచబడినవాటి కింద ఉన్నాయి.
   *[other] అన్‌డూ { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $restored } తిరిగి పెట్టాక ఆగింది. మిగిలినవి ఇంకా దాచబడినవాటి కింద ఉన్నాయి.
}

journal.window.entities.link.label.back.to.entities = ← ఎంటిటీలకు వెనుకకు

journal.window.entities.link.label.edit = సవరించండి

journal.window.entities.link.label.in.all.facets = ఫాసెట్‌లన్నిటిలో →

journal.window.entities.list.heading.facet = { $facet_name } లో ఎంటిటీలు

journal.window.entities.list.last.mentioned.short.day = చివరిసారి ప్రస్తావన { $day }

journal.window.entities.list.page.range = { $total } లో { $first }–{ $last }

journal.window.entities.list.results.count = { $matched_quantity ->
    [one] { $total_quantity ->
        [one] { $total } ఎంటిటీలో
       *[other] { $total } ఎంటిటీలలో
    } { $matched }
   *[other] { $total_quantity ->
        [one] { $total } ఎంటిటీలో
       *[other] { $total } ఎంటిటీలలో
    } { $matched }
}

journal.window.entities.list.select.all.count = { $total_quantity ->
    [one] అన్నీ { $total } ఎంచుకోండి
   *[other] అన్నీ { $total } ఎంచుకోండి
}

journal.window.entities.list.selected.count = { $selected_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $selected } టిక్ అయింది
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $selected } టిక్ అయింది
}

journal.window.entities.list.sort.name = పేరు ప్రకారం

journal.window.entities.list.sort.recent = ఇటీవలి

journal.window.entities.list.sort.type = రకం ప్రకారం

journal.window.entities.merge.confirm.selection = { $selected_quantity ->
    [one] { $selected } జత చేయండి
   *[other] { $selected } జత చేయండి
}

journal.window.entities.merge.evidence.meeting.photos = సమావేశ ఫోటోలు ఇవి ఒకే వ్యక్తి కావచ్చని సూచిస్తున్నాయి.

journal.window.entities.merge.keep.done = { $total_quantity ->
    [one] 1 జత వేరుగా ఉంచబడింది.
   *[other] { $total } జతలు వేరుగా ఉంచబడ్డాయి.
}

journal.window.entities.merge.keep.partial = { $failed_quantity ->
    [one] { $total_quantity ->
        [one] { $total } జతలో
       *[other] { $total } జతలలో
    } { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } వేరుగా ఉంచబడింది. 1 ఇంకా జాబితాలో ఉంది.
   *[other] { $total_quantity ->
        [one] { $total } జతలో
       *[other] { $total } జతలలో
    } { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } వేరుగా ఉంచబడింది. { $failed } ఇంకా జాబితాలో ఉన్నాయి.
}

journal.window.entities.merge.keep.selection = { $selected_quantity ->
    [one] { $selected } వేరుగా ఉంచండి
   *[other] { $selected } వేరుగా ఉంచండి
}

journal.window.entities.merge.merge.done = { $total_quantity ->
    [one] 1 జత చేయబడింది.
   *[other] { $total } జతలు చేయబడ్డాయి.
}

journal.window.entities.merge.merge.partial = { $failed_quantity ->
    [one] { $total_quantity ->
        [one] { $total } జతలో
       *[other] { $total } జతలలో
    } { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } జత చేయబడింది. 1 జత చేయలేకపోయింది.
   *[other] { $total_quantity ->
        [one] { $total } జతలో
       *[other] { $total } జతలలో
    } { $processed_quantity ->
        [one] { $processed }
       *[other] { $processed }
    } జత చేయబడింది. { $failed } జత చేయలేకపోయింది.
}

journal.window.entities.merge.pair.confirm.aria = { $pairs_quantity ->
    [one] 1 జతను జత చేయాలా?
   *[other] { $pairs } జతలను జత చేయాలా?
}

journal.window.entities.merge.pair.confirm.heading = { $pairs_quantity ->
    [one] 1 జతను జత చేయాలా?
   *[other] { $pairs } జతలను జత చేయాలా?
}

journal.window.entities.merge.preview.direction = { $source_name } { $target_name } లోకి జత అవుతుంది

journal.window.entities.merge.progress = { $done_quantity ->
    [one] జత చేస్తోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $done }
   *[other] జత చేస్తోంది… { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $done }
}

journal.window.entities.merge.progress.initial = { $total_quantity ->
    [one] జత చేస్తోంది… { $total } లో 0
   *[other] జత చేస్తోంది… { $total } లో 0
}

journal.window.entities.merge.unmerged.pair.count = { $pairs_quantity ->
    [one] మరో 1 జత జత చేయకుండా జాబితా నుండి వెళ్తుంది: అందులో పైన జత అయిన ఎంటిటీ ఉంది.
   *[other] మరో { $pairs } జతలు జత చేయకుండా జాబితా నుండి వెళ్తాయి: ప్రతిదాంట్లో పైన జత అయిన ఎంటిటీ ఉంది.
}

journal.window.entities.message.it.may.already.be.back.in = ఇది ఇప్పటికే మీ జర్నల్‌లోకి తిరిగి వచ్చి ఉండవచ్చు. సరిచూడటానికి మళ్ళీ లోడ్ చేయండి.

journal.window.entities.message.it.may.already.be.erased.reload = ఇది ఇప్పటికే తుడిచివేయబడి ఉండవచ్చు. సరిచూడటానికి మళ్ళీ లోడ్ చేయండి.

journal.window.entities.message.it.may.already.be.in.the = ఇది ఇప్పటికే ట్రాష్‌లో ఉండవచ్చు. సరిచూడటానికి మళ్ళీ లోడ్ చేయండి.

journal.window.entities.message.it.s.back.in.your.journal = ఇది మళ్ళీ మీ జర్నల్‌లో ఉంది.

journal.window.entities.message.it.s.in.the.trash.you = ఇది ట్రాష్‌లో ఉంది. అక్కడి నుండి మళ్ళీ ప్రయత్నించవచ్చు.

journal.window.entities.message.it.s.still.in.the.trash = ఇది ఇంకా ట్రాష్‌లో ఉంది. అక్కడి నుండి మళ్ళీ ప్రయత్నించవచ్చు.

journal.window.entities.message.it.s.still.in.your.journal = ఇది ఇంకా మీ జర్నల్‌లో ఉంది.

journal.window.entities.message.they.may.already.be.back.in = ఇవి ఇప్పటికే మీ జర్నల్‌లోకి తిరిగి వచ్చి ఉండవచ్చు. సరిచూడటానికి మళ్ళీ లోడ్ చేయండి.

journal.window.entities.message.they.may.already.be.erased.reload = ఇవి ఇప్పటికే తుడిచివేయబడి ఉండవచ్చు. సరిచూడటానికి మళ్ళీ లోడ్ చేయండి.

journal.window.entities.message.they.may.already.be.in.the = ఇవి ఇప్పటికే ట్రాష్‌లో ఉండవచ్చు. సరిచూడటానికి మళ్ళీ లోడ్ చేయండి.

journal.window.entities.message.they.re.back.in.your.journal = ఇవి మళ్ళీ మీ జర్నల్‌లో ఉన్నాయి.

journal.window.entities.message.they.re.still.in.the.trash = ఇవి ఇంకా ట్రాష్‌లో ఉన్నాయి. అక్కడి నుండి మళ్ళీ ప్రయత్నించవచ్చు.

journal.window.entities.notes.count = { $notes_quantity ->
    [one] 1 నోట్
   *[other] { $notes } నోట్‌లు
}

journal.window.entities.notes.none = నోట్‌లు లేవు

journal.window.entities.observation.coverage = { $observations_quantity ->
    [one] { $observations } పరిశీలన { $days_quantity ->
        [one] { $days } రోజు అంతటా
       *[other] { $days } రోజుల అంతటా
    }
   *[other] { $observations } పరిశీలనలు { $days_quantity ->
        [one] { $days } రోజు అంతటా
       *[other] { $days } రోజుల అంతటా
    }
}

journal.window.entities.observation.disclosure = { $visible_quantity ->
    [one] { $total_quantity ->
        [one] { $total } క్షణంలో
       *[other] { $total } క్షణాలలో
    } ఇటీవలి { $visible } చూపిస్తోంది
   *[other] { $total_quantity ->
        [one] { $total } క్షణంలో
       *[other] { $total } క్షణాలలో
    } ఇటీవలి { $visible } చూపిస్తోంది
}

journal.window.entities.observation.grid.count = { $quantity ->
    [one] 1 క్షణం
   *[other] { $count } క్షణాలు
}

journal.window.entities.observation.latest.day = ఇటీవలి రోజు: { $day }

journal.window.entities.observation.none.with.entity = { $entity_name } తో ఇంకా ఏమీ లేదు.

journal.window.entities.observation.range = { $start_day } – { $end_day }

journal.window.entities.observation.show.all = { $observations_quantity ->
    [one] { $observations } పరిశీలన చూపండి
   *[other] { $observations } పరిశీలనలు అన్నీ చూపండి
}

journal.window.entities.response.alias.conflict = అలియాస్ '{ $alias }' ఎంటిటీ '{ $conflicting_name }' తో ఢీ అవుతుంది.

journal.window.entities.response.alias.conflict.rename = అలియాస్ '{ $alias }' ఎంటిటీ '{ $conflicting_name }' తో ఢీ అవుతుంది

journal.window.entities.response.ambiguity.dismiss.status = స్థితి { $ambiguity_status } ఉన్న అయోమయాన్ని వదిలివేయలేం

journal.window.entities.response.ambiguity.file.corrupt = అయోమయం ఫైల్ { $file_path } లో పాడైన వరుస ఉంది

journal.window.entities.response.candidate.facet.already.exists = ఫాసెట్ '{ $facet }' ఇప్పటికే ఉంది

journal.window.entities.response.candidate.facet.candidate.accept.failed = అభ్యర్థిని అంగీకరించలేకపోయింది

journal.window.entities.response.candidate.facet.candidate.dismiss.failed = అభ్యర్థిని వదిలివేయలేకపోయింది

journal.window.entities.response.candidate.facet.candidate.read.failed = అభ్యర్థిని చదవలేకపోయింది

journal.window.entities.response.candidate.facet.facet.creation.failed = ఫాసెట్‌ను సృష్టించలేకపోయింది

journal.window.entities.response.candidate.facet.facet.lookup.failed = ఫాసెట్ లుకప్ సాధ్యం కాలేదు

journal.window.entities.response.candidate.facet.slug.invalid = ఫాసెట్ పేరు '{ $facet }' చెల్లదు: అంతా చిన్న అక్షరాలే అయి ఉండాలి, అక్షరంతో మొదలవ్వాలి, అక్షరాలు, అంకెలు, హైఫన్‌లు లేదా అండర్‌స్కోర్‌లు మాత్రమే ఉండాలి

journal.window.entities.response.candidate.not.found = అభ్యర్థి దొరకలేదు

journal.window.entities.response.candidate.record.missing = అభ్యర్థి జతకు సరిపోయే క్రియాశీల నమోదు లేదు

journal.window.entities.response.candidate.status.accept = స్థితి{ $slot1 }{ $status } ఉన్న అభ్యర్థిని అంగీకరించలేం

journal.window.entities.response.candidate.status.dismiss = స్థితి{ $slot1 }{ $status } ఉన్న అభ్యర్థిని వదిలివేయలేం

journal.window.entities.response.candidate.status.preview = స్థితి{ $slot1 }{ $status } ఉన్న అభ్యర్థిని ప్రివ్యూ చూడలేం

journal.window.entities.response.delete.refusal.ambiguities.unreadable = మీ జర్నల్ అయోమయ నమోదులను చదవలేకపోయింది

journal.window.entities.response.delete.refusal.bad.request = ఆ తొలగింపును చదవలేకపోయింది

journal.window.entities.response.delete.refusal.busy = మీ జర్నల్ బిజీగా ఉంది; ఒక క్షణం తర్వాత మళ్ళీ ప్రయత్నించండి

journal.window.entities.response.delete.refusal.failed = మీ జర్నల్ దీన్ని ట్రాష్‌కు తరలించలేకపోయింది

journal.window.entities.response.delete.refusal.gone = ఆ ఎంటిటీ మీ జర్నల్‌లో లేదు

journal.window.entities.response.delete.refusal.link.unreadable = ఒక ఫాసెట్ లింక్‌ను చదవలేకపోయింది

journal.window.entities.response.delete.refusal.principal = మిమ్మల్ని మీరు తొలగించలేరు

journal.window.entities.response.delete.refusal.record.damaged = తొలగించిన ఎంటిటీల నమోదును మీ జర్నల్ అప్‌డేట్ చేయలేకపోయింది

journal.window.entities.response.detail.access.denied = యాక్సెస్ నిరాకరించబడింది

journal.window.entities.response.detail.after.id.and.offset.cannot.be.used.together = { $slot1 } మరియు ఆఫ్‌సెట్ కలిపి వాడలేం

journal.window.entities.response.detail.after.id.must.be.a.non.negative.integer = { $slot1 } 0 కంటే తక్కువ కాని పూర్ణాంకం అయి ఉండాలి

journal.window.entities.response.detail.aka.is.required = ఇలా కూడా అవసరం

journal.window.entities.response.detail.ambiguity.dismiss.failed = అయోమయాన్ని వదిలివేయలేకపోయింది

journal.window.entities.response.detail.ambiguity.id.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.ambiguity.not.found = అయోమయం దొరకలేదు

journal.window.entities.response.detail.ambiguity.read.failed = అయోమయాన్ని చదవలేకపోయింది

journal.window.entities.response.detail.ambiguity.resolve.failed = అయోమయాన్ని పరిష్కరించలేకపోయింది

journal.window.entities.response.detail.batch.block.failed = బ్యాచ్ బ్లాక్ సాధ్యం కాలేదు

journal.window.entities.response.detail.batch.unblock.failed = బ్యాచ్ బ్లాక్ తీసివేయడం సాధ్యం కాలేదు

journal.window.entities.response.detail.body.is.not.valid.json = బాడీ చెల్లుబాటు అయ్యే { $slot1 } కాదు

journal.window.entities.response.detail.body.must.be.a.json.object = బాడీ { $slot1 } ఆబ్జెక్ట్ అయి ఉండాలి

journal.window.entities.response.detail.candidate.read.failed = అభ్యర్థిని చదవలేకపోయింది

journal.window.entities.response.detail.content.is.required = కంటెంట్ అవసరం

journal.window.entities.response.detail.couldn.t.hide.entity = ఎంటిటీని దాచలేకపోయింది

journal.window.entities.response.detail.couldn.t.unhide.entity = ఎంటిటీని మళ్ళీ చూపలేకపోయింది

journal.window.entities.response.detail.day.is.required = రోజు అవసరం

journal.window.entities.response.detail.description.is.required = వివరణ అవసరం

journal.window.entities.response.detail.detected.entity.activity.is.unavailable = గుర్తించిన ఎంటిటీ కార్యకలాపం అందుబాటులో లేదు

journal.window.entities.response.detail.detected.entity.not.found = గుర్తించిన ఎంటిటీ దొరకలేదు

journal.window.entities.response.detail.detected.entity.save.failed = గుర్తించిన ఎంటిటీని సేవ్ చేయలేకపోయింది

journal.window.entities.response.detail.detected.entity.update.failed = గుర్తించిన ఎంటిటీని అప్‌డేట్ చేయలేకపోయింది

journal.window.entities.response.detail.detected.read.failed = గుర్తించినవాటిని చదవలేకపోయింది

journal.window.entities.response.detail.detections.can.t.be.shown.here.which.names.are = గుర్తించడాలు ఇక్కడ చూపలేం: ఏ పేర్లు దాచబడ్డాయో చదవలేకపోయింది

journal.window.entities.response.detail.entity.alias.update.failed = ఎంటిటీ అలియాస్‌ను అప్‌డేట్ చేయలేకపోయింది

journal.window.entities.response.detail.entity.already.exists = ఎంటిటీ ఇప్పటికే ఉంది

journal.window.entities.response.detail.entity.already.exists.in.destination.facet.use.merge.to = గమ్యం ఫాసెట్‌లో ఎంటిటీ ఇప్పటికే ఉంది. జత చేయడానికి { $slot1 } వాడండి.

journal.window.entities.response.detail.entity.busy = ఎంటిటీ బిజీగా ఉంది

journal.window.entities.response.detail.entity.create.failed = ఎంటిటీని సృష్టించలేకపోయింది

journal.window.entities.response.detail.entity.data.directory.not.found.in.source.facet = మూలం ఫాసెట్‌లో ఎంటిటీ డేటా డైరెక్టరీ దొరకలేదు.

journal.window.entities.response.detail.entity.description.update.failed = ఎంటిటీ వివరణను అప్‌డేట్ చేయలేకపోయింది

journal.window.entities.response.detail.entity.detach.failed = ఎంటిటీని విడదీయలేకపోయింది

journal.window.entities.response.detail.entity.hidden = ఎంటిటీ దాచబడింది

journal.window.entities.response.detail.entity.id.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.entity.is.required = ఎంటిటీ అవసరం

journal.window.entities.response.detail.entity.move.failed = ఎంటిటీని తరలించలేకపోయింది

journal.window.entities.response.detail.entity.name.is.invalid = ఎంటిటీ పేరు చెల్లదు

journal.window.entities.response.detail.entity.not.found = ఎంటిటీ దొరకలేదు

journal.window.entities.response.detail.entity.not.found.in.facet = ఫాసెట్‌లో ఎంటిటీ దొరకలేదు

journal.window.entities.response.detail.entity.not.found.variant2 = ఎంటిటీ దొరకలేదు

journal.window.entities.response.detail.entity.read.failed = ఎంటిటీని చదవలేకపోయింది

journal.window.entities.response.detail.entity.restore.failed = ఎంటిటీని రీస్టోర్ చేయలేకపోయింది

journal.window.entities.response.detail.entity.search.index.is.busy = ఎంటిటీ సెర్చ్ ఇండెక్స్ బిజీగా ఉంది

journal.window.entities.response.detail.entity.search.index.is.stale = ఎంటిటీ సెర్చ్ ఇండెక్స్ పాతబడింది

journal.window.entities.response.detail.entity.search.index.is.unavailable = ఎంటిటీ సెర్చ్ ఇండెక్స్ అందుబాటులో లేదు

journal.window.entities.response.detail.entity.update.failed = ఎంటిటీని అప్‌డేట్ చేయలేకపోయింది

journal.window.entities.response.detail.entity.with.this.name.already.exists.in.facet = ఈ పేరుతో ఎంటిటీ ఫాసెట్‌లో ఇప్పటికే ఉంది

journal.window.entities.response.detail.evidence.is.required = ఆధారం అవసరం

journal.window.entities.response.detail.exclude.name.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.facet.is.required = ఫాసెట్ అవసరం

journal.window.entities.response.detail.facet.not.found = ఫాసెట్ దొరకలేదు

journal.window.entities.response.detail.facet.read.failed = ఫాసెట్‌ను చదవలేకపోయింది

journal.window.entities.response.detail.from.facet.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.grid.read.failed = గ్రిడ్‌ను చదవలేకపోయింది

journal.window.entities.response.detail.group.resolve.failed = సమూహాన్ని పరిష్కరించలేకపోయింది

journal.window.entities.response.detail.hidden.must.be.include.exclude.or.only = { $slot1 } { $slot2 }, { $slot3 } లేదా { $slot4 } అయి ఉండాలి

journal.window.entities.response.detail.history.read.failed = చరిత్రను చదవలేకపోయింది

journal.window.entities.response.detail.history.version.was.not.found = చరిత్ర వెర్షన్ దొరకలేదు

journal.window.entities.response.detail.ids.elements.must.be.strings = ids అంశాలు స్ట్రింగ్‌లు అయి ఉండాలి

journal.window.entities.response.detail.ids.is.required = ids అవసరం

journal.window.entities.response.detail.ids.must.be.an.array = ids అర్రే అయి ఉండాలి

journal.window.entities.response.detail.index.plate.task.failed = ఇండెక్స్ ప్లేట్ పని సాధ్యం కాలేదు

journal.window.entities.response.detail.invalid.detected.entity = గుర్తించిన ఎంటిటీ చెల్లదు

journal.window.entities.response.detail.invalid.target.kind = టార్గెట్ రకం చెల్లదు

journal.window.entities.response.detail.journal.read.failed = జర్నల్‌ను చదవలేకపోయింది

journal.window.entities.response.detail.limit.bounded.two.hundred = పరిమితి 0 నుండి 200 మధ్య ఉండాలి

journal.window.entities.response.detail.limit.is.required = పరిమితి అవసరం

journal.window.entities.response.detail.limit.must.be.an.integer = పరిమితి పూర్ణాంకం అయి ఉండాలి

journal.window.entities.response.detail.member.ids.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.member.ids.must.contain.only.strings = { $slot1 } లో స్ట్రింగ్‌లు మాత్రమే ఉండాలి

journal.window.entities.response.detail.member.ids.must.not.be.empty = { $slot1 } ఖాళీగా ఉండకూడదు

journal.window.entities.response.detail.merge.candidate.accept.failed = జత అభ్యర్థి అంగీకరించడం విఫలమైంది

journal.window.entities.response.detail.merge.candidate.dismiss.failed = జత అభ్యర్థి వదిలివేయడం విఫలమైంది

journal.window.entities.response.detail.merge.candidate.record.failed = జత అభ్యర్థి నమోదు విఫలమైంది

journal.window.entities.response.detail.merge.commit.failed = జత కమిట్ విఫలమైంది

journal.window.entities.response.detail.merge.preview.failed = జత ప్రివ్యూ విఫలమైంది

journal.window.entities.response.detail.missing.name.key = { $slot1 } లేదు

journal.window.entities.response.detail.name.is.required = పేరు అవసరం

journal.window.entities.response.detail.no.data.provided = డేటా ఇవ్వలేదు

journal.window.entities.response.detail.observation.content.cannot.be.empty = పరిశీలన కంటెంట్ ఖాళీగా ఉండకూడదు

journal.window.entities.response.detail.observation.entity.lookup.failed = పరిశీలన ఎంటిటీ లుకప్ విఫలమైంది

journal.window.entities.response.detail.offset.must.be.a.non.negative.integer = ఆఫ్‌సెట్ ఋణం కాని పూర్ణాంకం అయి ఉండాలి

journal.window.entities.response.detail.offset.must.not.be.negative = ఆఫ్‌సెట్ ఋణం అయి ఉండకూడదు

journal.window.entities.response.detail.old.name.and.new.name.are.required = { $slot1 } మరియు { $slot2 } అవసరం

journal.window.entities.response.detail.operation.is.required = ఆపరేషన్ అవసరం

journal.window.entities.response.detail.operation.must.be.a.string = ఆపరేషన్ స్ట్రింగ్ అయి ఉండాలి

journal.window.entities.response.detail.recorded.entity.choice.is.absent.from.this.facet = నమోదు అయిన ఎంటిటీ ఎంపిక ఈ ఫాసెట్‌లో లేదు

journal.window.entities.response.detail.recorded.entity.choice.is.hidden = నమోదు అయిన ఎంటిటీ ఎంపిక దాచబడింది

journal.window.entities.response.detail.request.body.could.not.be.read = అభ్యర్థన వచనం చదవలేకపోయింది

journal.window.entities.response.detail.resolution.task.failed = పరిష్కార పని విఫలమైంది

journal.window.entities.response.detail.restore.is.not.available = రీస్టోర్ అందుబాటులో లేదు

journal.window.entities.response.detail.review.item.not.found = సమీక్ష అంశం దొరకలేదు

journal.window.entities.response.detail.review.read.failed = సమీక్ష చదవడం విఫలమైంది

journal.window.entities.response.detail.review.restore.failed = సమీక్ష రీస్టోర్ విఫలమైంది

journal.window.entities.response.detail.review.sweep.failed = సమీక్ష స్వీప్ విఫలమైంది

journal.window.entities.response.detail.revision.is.required = రివిజన్ అవసరం

journal.window.entities.response.detail.since.must.be.a.yyyymmdd.day = since ఒక { $slot1 } రోజు అయి ఉండాలి

journal.window.entities.response.detail.sort.must.be.activity.or.name = { $slot1 } { $slot2 } లేదా { $slot3 } అయి ఉండాలి

journal.window.entities.response.detail.source.and.target.resolve.to.the.same.entity = మూలం, టార్గెట్ ఒకే ఎంటిటీకి రిజల్వ్ అవుతాయి.

journal.window.entities.response.detail.source.is.required = మూలం అవసరం

journal.window.entities.response.detail.source.slug.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.status.must.be.open.or.resolved = { $slot1 } { $slot2 } లేదా { $slot3 } అయి ఉండాలి

journal.window.entities.response.detail.suggestions.are.busy.try.again = సూచనలు బిజీగా ఉన్నాయి; మళ్ళీ ప్రయత్నించండి

journal.window.entities.response.detail.target.is.required = టార్గెట్ అవసరం

journal.window.entities.response.detail.target.slug.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.the.name.couldn.t.be.changed.so.nothing.changed = పేరు మార్చలేకపోయింది, అందుకే ఏమీ మారలేదు

journal.window.entities.response.detail.the.names.hidden.in.this.facet.can.t.be = ఈ ఫాసెట్‌లో దాచబడిన పేర్లు చదవలేకపోయాయి, అందుకే ఏమీ మారలేదు

journal.window.entities.response.detail.this.entity.is.hidden.so.its.names.can.t = ఈ ఎంటిటీ దాచబడింది, అందుకే దాన్ని మళ్ళీ చూపే వరకు పేర్లు మారవు

journal.window.entities.response.detail.this.entity.is.hidden.unhide.it.in.the.entities = ఈ ఎంటిటీ దాచబడింది. ముందు ఎంటిటీల యాప్‌లో దాన్ని మళ్ళీ చూపండి.

journal.window.entities.response.detail.to.facet.is.required = { $slot1 } అవసరం

journal.window.entities.response.detail.type.and.name.are.required = రకం, పేరు అవసరం

journal.window.entities.response.detail.type.is.required = రకం అవసరం

journal.window.entities.response.detail.type.must.be.a.canonical.entity.type.or.other = రకం క్యానానికల్ ఎంటిటీ రకం లేదా ఇతరం అయి ఉండాలి

journal.window.entities.response.detail.version.id.is.required = { $slot1 } అవసరం

journal.window.entities.response.entity.exists = ఎంటిటీ '{ $entity_name }' ఇప్పటికే ఉంది

journal.window.entities.response.entity.merged.permanent = '{ $merged_id }' '{ $survivor_id }' లోకి జత అయింది. జతలను అన్‌డూ చేయలేం, అందుకే ఆ పేరును కొత్త ఎంటిటీగా చేర్చలేం; బదులుగా '{ $survivor_id }' వాడండి.

journal.window.entities.response.entity.missing = ఎంటిటీ '{ $entity_id }' దొరకలేదు

journal.window.entities.response.entity.type.invalid = చెల్లని ఎంటిటీ రకం { $entity_type }

journal.window.entities.response.entity.type.invalid.query = చెల్లని ఎంటిటీ రకం: { $entity_type }

journal.window.entities.response.entity.type.invalid.write = చెల్లని ఎంటిటీ రకం '{ $entity_type }'

journal.window.entities.response.facet.entity.missing = '{ $entity_name }' ఈ ఫాసెట్‌లో లేదు. ముందు దాన్ని ఇక్కడ చేర్చండి, ఆపై దాని గురించి నోట్‌లు చేర్చండి.

journal.window.entities.response.ids.count.invalid = చెల్లని ids సంఖ్య: { $id_count }

journal.window.entities.response.merge.refusal.both.principal = వీటిని జత చేయలేం: రెండూ మీరుగా గుర్తు ఉన్నాయి.

journal.window.entities.response.merge.refusal.person.type = వ్యక్తి కాని దానిలోకి వ్యక్తిని జత చేయలేం.

journal.window.entities.response.merge.refusal.principal.type = వ్యక్తి కాని దానిలోకి మిమ్మల్ని జత చేయలేం.

journal.window.entities.response.merge.refusal.same.entity = మూలం, టార్గెట్ వేరు వేరు ఎంటిటీలు అయి ఉండాలి.

journal.window.entities.response.merge.refusal.source.hidden = దాచబడిన దాన్ని జత చేయలేం. ముందు దాన్ని మళ్ళీ చూపండి.

journal.window.entities.response.merge.refusal.source.missing = మూల ఎంటిటీ దొరకలేదు:{ $slot1 }{ $id }

journal.window.entities.response.merge.refusal.target.hidden = దాచబడిన దానిలోకి జత చేయలేం. ముందు దాన్ని మళ్ళీ చూపండి.

journal.window.entities.response.merge.refusal.target.missing = టార్గెట్ ఎంటిటీ దొరకలేదు:{ $slot1 }{ $id }

journal.window.entities.response.merge.repair = ఈ జతను బాగుచేయడానికి ఆపరేటర్‌ను సంప్రదించండి.

journal.window.entities.response.operation.already.undone = ఆపరేషన్ { $operation_id } ఇప్పటికే అన్‌డూ అయింది

journal.window.entities.response.operation.id.invalid = చెల్లని ఆపరేషన్ id: { $operation_id }

journal.window.entities.response.operation.limit.invalid = చెల్లని పరిమితి: { $limit }

journal.window.entities.response.photo.entity.not.found = ఆ వ్యక్తి దొరకలేదు.

journal.window.entities.response.photo.face.action.changed = పేజీ లోడ్ అయిన తర్వాత ఇది మారింది. పేజీని రిఫ్రెష్ చేయండి.

journal.window.entities.response.photo.face.not.found = ఆ ఫోటో ఇక అందుబాటులో లేదు.

journal.window.entities.response.photo.faces.busy = ముఖ డేటా బిజీగా ఉంది. కాసేపట్లో మళ్ళీ ప్రయత్నించండి.

journal.window.entities.response.photo.faces.disabled = ప్రొఫైల్ చిత్రం మార్చే ముందు ప్రొఫైల్ చిత్రం సూచనలను ఆన్ చేయండి.

journal.window.entities.response.photo.faces.enabled = ముఖ డేటా తుడిచివేసే ముందు ప్రొఫైల్ చిత్రం సూచనలను ఆఫ్ చేయండి.

journal.window.entities.response.photo.faces.read.failed = ఆ ఫోటోలు చదవలేకపోయాయి. మళ్ళీ ప్రయత్నించండి.

journal.window.entities.response.photo.request.invalid = ఆ ఫోటో అభ్యర్థన చదవలేకపోయింది.

journal.window.entities.response.principal.protected = ప్రధాన ఎంటిటీ ఈ ఆపరేషన్ నుండి రక్షించబడింది: { $entity_id }

journal.window.entities.response.query.integer.bounded = { $parameter_name } 0 నుండి { $maximum } మధ్య ఉండాలి

journal.window.entities.response.query.integer.required = { $parameter_name } పూర్ణాంకం అయి ఉండాలి

journal.window.entities.response.refused = ఎంటిటీ అభ్యర్థన నిరాకరించబడింది

journal.window.entities.response.summary.limit.bounded = పరిమితి 0 నుండి { $maximum } మధ్య ఉండాలి

journal.window.entities.scope.showing.facet = { $facet_name } చూపిస్తోంది ⌄

journal.window.entities.scope.showing.journal = మీ జర్నల్ మొత్తం చూపిస్తోంది ⌄

journal.window.entities.search.finding.names = "{ $query }" కు సరిపోయే ఎంటిటీలను వెతుకుతోంది…

journal.window.entities.search.finding.type = { $entity_type } ఎంటిటీలను వెతుకుతోంది…

journal.window.entities.search.finding.type.and.names = "{ $query }" కు సరిపోయే { $entity_type } ఎంటిటీలను వెతుకుతోంది…

journal.window.entities.search.none.match = "{ $query }" కు ఏ ఎంటిటీలూ సరిపోలేదు.

journal.window.entities.search.placeholder.entity.count = { $total_quantity ->
        [one] { $total } ఎంటిటీని వెతకండి
       *[other] { $total } ఎంటిటీలను వెతకండి
    }

journal.window.entities.section.heading.detach = విడదీయండి

journal.window.entities.section.heading.no.entities.yet = ఇంకా ఎంటిటీలు లేవు

journal.window.entities.state.edit.loading = లోడ్ అవుతోంది…

journal.window.entities.state.edit.unavailable = అందుబాటులో లేదు

journal.window.entities.text.couldn.t.hide.them = వాటిని దాచలేకపోయింది.

journal.window.entities.text.hiding.stopped.partway.undo.puts.back = దాచడం మధ్యలో ఆగింది. అన్‌డూ దాచినవన్నీ తిరిగి పెడుతుంది.

journal.window.entities.text.loading = లోడ్ అవుతోంది…

journal.window.entities.text.loading.entities = ఎంటిటీలు లోడ్ అవుతున్నాయి…

journal.window.entities.text.undo.stopped.before.putting.any.back = ఏదీ తిరిగి పెట్టకముందే అన్‌డూ ఆగింది. అవి ఇంకా దాచబడిన వాటి కింద ఉన్నాయి.

journal.window.entities.tidy.duplicate.count = { $pairs_quantity ->
        [one] { $pairs } సాధ్యమైన నకలు
       *[other] { $pairs } సాధ్యమైన నకలులు
    }

journal.window.entities.tidy.group.load.error = ఈ సమూహాన్ని లోడ్ చేయలేకపోయింది.

journal.window.entities.tidy.id.count = { $entities_quantity ->
        [one] { $entities } ID లాగా ఉంది
       *[other] { $entities } ID లాగా ఉన్నాయి
    }

journal.window.entities.title.couldn.t.erase.those = వాటిని తుడిచివేయలేకపోయింది

journal.window.entities.title.couldn.t.move.that.to.the = దాన్ని ట్రాష్‌కు తరలించలేకపోయింది

journal.window.entities.title.couldn.t.move.those.to.the = వాటిని ట్రాష్‌కు తరలించలేకపోయింది

journal.window.entities.title.couldn.t.recover.those = వాటిని రీస్టోర్ చేయలేకపోయింది

journal.window.entities.title.delete.this.entity = ఈ ఎంటిటీని తొలగించాలా?

journal.window.entities.title.erase.this.entity.now = ఈ ఎంటిటీని ఇప్పుడే తుడిచివేయాలా?

journal.window.entities.title.moved.to.the.trash = ట్రాష్‌కు తరలించబడింది

journal.window.entities.title.your.journal.didn.t.answer = మీ జర్నల్ సమాధానం ఇవ్వలేదు

journal.window.entities.tooltip.add.to.facet = ఫాసెట్‌కు చేర్చండి

journal.window.entities.tooltip.cannot.hide.yourself = మిమ్మల్ని మీరు దాచలేరు

journal.window.entities.tooltip.entity.is.hidden.and.detached.from = ఎంటిటీ దాచబడింది, ఈ ఫాసెట్ నుండి విడిపోయింది

journal.window.entities.tooltip.entity.type = రకం: { $entity_type }

journal.window.entities.tooltip.facet.name = ఫాసెట్: { $facet_name }

journal.window.entities.tooltip.has.voiceprint = వాయిస్‌ప్రింట్ ఉంది

journal.window.entities.tooltip.last.mentioned.full.day = చివరిసారి ప్రస్తావన: { $day }

journal.window.entities.tooltip.last.mentioned.short.day = చివరిసారి ప్రస్తావన { $day }

journal.window.entities.tooltip.mention.days = { $mentions_quantity ->
        [one] { $mentions } ప్రస్తావన: గత 30 రోజుల్లో గుర్తించిన రోజులు
       *[other] { $mentions } ప్రస్తావనలు: గత 30 రోజుల్లో గుర్తించిన రోజులు
    }

journal.window.entities.tooltip.this.entity.is.hidden = ఈ ఎంటిటీ దాచబడింది

journal.window.entities.tooltip.this.is.you = ఇది మీరు

journal.window.entities.tooltip.this.is.your.entity.in.this = ఇది ఈ ఫాసెట్‌లో మీ ఎంటిటీ

journal.window.entities.tooltip.unhide.this.entity = ఈ ఎంటిటీని మళ్ళీ చూపండి

journal.window.entities.tooltip.view.in.facet = { $facet_name } లో చూడటానికి క్లిక్ చేయండి

journal.window.entities.trust.history.change = { $previous_identity } → { $updated_identity }

journal.window.entities.trust.history.event = { $event_kind } · { $event_time } · { $entity_name }

journal.window.entities.trust.history.suggested.by = { $agent_name } నుండి సూచన

journal.window.entities.trust.identity.details = { $entity_name } · { $entity_type } · ఇలా కూడా: { $aliases }

journal.window.entities.trust.merge.confirm = { $target_name } లోకి { $source_name } జత చేయాలా? జతను అన్‌డూ చేయలేం.

journal.window.entities.trust.merge.preview.counts = { $voice_samples_quantity ->
        [one] చేర్చిన గొంతు నమూనాలు: { $voice_samples } · జత తర్వాత నమూనాలు: { $voice_total_quantity ->
        [one] { $voice_total }
       *[other] { $voice_total }
    } · జత అవుతున్న పేరు కింద నోట్‌లు: { $notes_quantity ->
        [one] { $notes }
       *[other] { $notes }
    } · మాట్లాడిన వారి లేబుల్‌లు మారాయి: { $speaker_labels_quantity ->
        [one] { $speaker_labels }
       *[other] { $speaker_labels }
    } · కార్యకలాప నమోదులు మారాయి: { $activities_quantity ->
        [one] { $activities }
       *[other] { $activities }
    }
       *[other] చేర్చిన గొంతు నమూనాలు: { $voice_samples } · జత తర్వాత నమూనాలు: { $voice_total_quantity ->
        [one] { $voice_total }
       *[other] { $voice_total }
    } · జత అవుతున్న పేరు కింద నోట్‌లు: { $notes_quantity ->
        [one] { $notes }
       *[other] { $notes }
    } · మాట్లాడిన వారి లేబుల్‌లు మారాయి: { $speaker_labels_quantity ->
        [one] { $speaker_labels }
       *[other] { $speaker_labels }
    } · కార్యకలాప నమోదులు మారాయి: { $activities_quantity ->
        [one] { $activities }
       *[other] { $activities }
    }
    }

journal.window.entities.trust.merge.preview.summary = { $aliases_quantity ->
        [one] చేర్చిన అలియాస్‌లు: { $aliases } · చేర్చిన ఈమెయిల్‌లు: { $emails_quantity ->
        [one] { $emails }
       *[other] { $emails }
    }
       *[other] చేర్చిన అలియాస్‌లు: { $aliases } · చేర్చిన ఈమెయిల్‌లు: { $emails_quantity ->
        [one] { $emails }
       *[other] { $emails }
    }
    }

journal.window.entities.trust.suggestion.details = { $source_name } → { $target_name } · { $facet_name }

journal.window.entities.visible.text.attach.to = జోడించండి

journal.window.entities.visible.text.attach.to.facet = ఫాసెట్‌కు జోడించండి

journal.window.entities.visible.text.attaching = జోడిస్తోంది…

journal.window.entities.visible.text.cancel = రద్దు చేయండి

journal.window.entities.visible.text.clear.selection = ఎంపికను క్లియర్ చేయండి

journal.window.entities.visible.text.comma.separated.list.of.alternative.names = ఆడియో ట్రాన్స్క్రిప్ట్ కోసం కామాతో వేరు చేసిన ప్రత్యామ్నాయ పేర్ల జాబితా

journal.window.entities.visible.text.connections = కనెక్షన్‌లు

journal.window.entities.visible.text.couldn.t.load.that.page.of = ఎంటిటీల ఆ పేజీని లోడ్ చేయలేకపోయింది.

journal.window.entities.visible.text.description = వివరణ

journal.window.entities.visible.text.detach = విడదీయండి

journal.window.entities.visible.text.detached = విడిపోయింది

journal.window.entities.visible.text.detected.entities = గుర్తించిన ఎంటిటీలు

journal.window.entities.visible.text.dots.are.the.facets.a.name = చుక్కలు ఒక పేరు కనిపించే ఫాసెట్‌లు.

journal.window.entities.visible.text.each.pair.below.becomes.one.entity = కింద ఉన్న ప్రతి జంట ఒక ఎంటిటీ అవుతుంది. జతను అన్‌డూ చేయలేం.

journal.window.entities.visible.text.edit.entity = ఎంటిటీని సవరించండి

journal.window.entities.visible.text.entities = ఎంటిటీలు

journal.window.entities.visible.text.facets = ఫాసెట్‌లు

journal.window.entities.visible.text.failed.to.save.changes = మార్పులు సేవ్ చేయలేకపోయింది

journal.window.entities.visible.text.found.by.ai.in.the.last = గత 30 రోజుల్లో AI కనుగొన్నవి. మీ జర్నల్‌కు చేర్చడానికి స్టార్ చేయండి

journal.window.entities.visible.text.has.voiceprint = వాయిస్‌ప్రింట్ ఉంది

journal.window.entities.visible.text.hidden = దాచబడింది

journal.window.entities.visible.text.hide = దాచండి

journal.window.entities.visible.text.hiding = దాస్తోంది…

journal.window.entities.visible.text.how.does.this.entity.relate.to = (ఈ ఎంటిటీ ఈ ఫాసెట్‌తో ఎలా సంబంధం కలిగి ఉంది?)

journal.window.entities.visible.text.loading = లోడ్ అవుతోంది…

journal.window.entities.visible.text.loading.entities = ఎంటిటీలు లోడ్ అవుతున్నాయి…

journal.window.entities.visible.text.loading.this.entity = ఈ ఎంటిటీ లోడ్ అవుతోంది…

journal.window.entities.visible.text.more.about.hidden = దాచబడింది గురించి మరింత

journal.window.entities.visible.text.name.is.required = పేరు అవసరం

journal.window.entities.visible.text.names.shaped.like.codes.file.names = కోడ్‌లు, ఫైల్ పేర్లు లేదా అభ్యర్థన సంఖ్యల లాగా ఉన్న పేర్లు. దేనికీ నోట్ లేదు.

journal.window.entities.visible.text.no.description.click.to.add = వివరణ లేదు. చేర్చడానికి క్లిక్ చేయండి…

journal.window.entities.visible.text.no.entities.match.your.search = మీరు వెతికిన దానికి ఏ ఎంటిటీలూ సరిపోలేదు.

journal.window.entities.visible.text.no.entity.matches.that.name = ఆ పేరుకు ఏ ఎంటిటీ సరిపోలేదు.

journal.window.entities.visible.text.not.attached = జోడించబడలేదు

journal.window.entities.visible.text.not.attached.to.any.facets.yet = ఇంకా ఏ ఫాసెట్‌కూ జోడించబడలేదు.

journal.window.entities.visible.text.not.now = ఇప్పుడు కాదు

journal.window.entities.visible.text.nothing.here = ఇక్కడ ఏమీ లేదు.

journal.window.entities.visible.text.only.in.facets.you.muted = మీరు మ్యూట్ చేసిన ఫాసెట్‌లలో మాత్రమే.

journal.window.entities.visible.text.pairs.that.might.be.the.same = ఒకే వ్యక్తి లేదా వస్తువు కావచ్చు అనే జంటలు. జతను అన్‌డూ చేయలేం.

journal.window.entities.visible.text.people.places.and.things.you.re = మీరు చూసుకుంటున్న వ్యక్తులు, ప్రదేశాలు, వస్తువులు

journal.window.entities.visible.text.review = సమీక్షించండి

journal.window.entities.visible.text.review.the.group = సమూహాన్ని సమీక్షించండి

journal.window.entities.visible.text.save = సేవ్ చేయండి

journal.window.entities.visible.text.save.changes = మార్పులు సేవ్ చేయండి

journal.window.entities.visible.text.saved = సేవ్ అయింది!

journal.window.entities.visible.text.saving = సేవ్ చేస్తోంది…

journal.window.entities.visible.text.select.a.facet.to.add.entities = మీ జర్నల్‌కు ఎంటిటీలు చేర్చడానికి ఒక ఫాసెట్‌ను ఎంచుకోండి.

journal.window.entities.visible.text.still.working = ఇంకా పని చేస్తోంది…

journal.window.entities.visible.text.they.move.to.hidden.where.you = అవి దాచబడిన వాటిలోకి తరలుతాయి, అక్కడ మీరు వాటిని మళ్ళీ చూపవచ్చు. కనెక్ట్ అయి ఉన్న ఏజెంట్‌లు వాటిని చేరలేవు. మీ సొంత ఎంటిటీ ఈ సమూహంలో ఎప్పుడూ ఉండదు.

journal.window.entities.visible.text.try.again = మళ్ళీ ప్రయత్నించండి

journal.window.entities.visible.text.undo = అన్‌డూ చేయండి

journal.window.entities.visible.text.unhide = మళ్ళీ చూపండి

journal.window.entities.visible.text.unhiding = మళ్ళీ చూపిస్తోంది…

journal.window.entities.visible.text.unticked.ones.stay.as.they.are = టిక్ చేయనివి ఉన్నట్లే ఉంటాయి.

journal.window.entities.visible.text.worth.a.look = చూడదగ్గవి

journal.window.entities.visible.text.you = మీరు

journal.window.entities.voice.heading.count = { $samples_quantity ->
        [one] గొంతు నమూనాలు ({ $samples })
       *[other] గొంతు నమూనాలు ({ $samples })
    }

journal.window.entities.voice.invalid.key = చెల్లని గొంతు నమూనా కీ

journal.window.entities.voice.sample.sentence = వాక్యం { $sentence_id }

journal.window.entities.window.explanation.couldn.t.keep.it.separate.it = దాన్ని వేరుగా ఉంచలేకపోయింది. అది ఇంకా జాబితాలో ఉంది.

journal.window.entities.window.explanation.couldn.t.keep.them.separate.they = వాటిని వేరుగా ఉంచలేకపోయింది. అవి ఇంకా జాబితాలో ఉన్నాయి.

journal.window.entities.window.explanation.couldn.t.merge.it.it.s = దాన్ని జత చేయలేకపోయింది. అది ఇంకా జాబితాలో ఉంది.

journal.window.entities.window.explanation.couldn.t.merge.them.they.re = వాటిని జత చేయలేకపోయింది. అవి ఇంకా జాబితాలో ఉన్నాయి.

journal.window.entities.window.explanation.couldn.t.search.for.a.merge = జత టార్గెట్ కోసం వెతకలేకపోయింది.

journal.window.entities.window.explanation.failed.to.load.observations = పరిశీలనలు లోడ్ చేయలేకపోయింది

journal.window.entities.window.explanation.look.like.an.id = ID లాగా ఉన్నాయి

journal.window.entities.window.explanation.this.entity.couldn.t.be.loaded = ఈ ఎంటిటీని లోడ్ చేయలేకపోయింది, అందుకే దాన్ని సవరించలేం.

journal.window.entities.window.state.text.couldn.t.attach.entity = ఎంటిటీని జోడించలేకపోయింది

journal.window.entities.window.state.text.couldn.t.detach.entity = ఎంటిటీని విడదీయలేకపోయింది

journal.window.entities.window.state.text.couldn.t.hide.entity = ఎంటిటీని దాచలేకపోయింది

journal.window.entities.window.state.text.couldn.t.re.attach.entity = ఎంటిటీని మళ్ళీ జోడించలేకపోయింది

journal.window.entities.window.state.text.couldn.t.save.description = వివరణను సేవ్ చేయలేకపోయింది

journal.window.entities.window.state.text.couldn.t.unhide.entity = ఎంటిటీని మళ్ళీ చూపలేకపోయింది

journal.window.entities.window.state.text.failed.to.attach = జోడించలేకపోయింది

journal.window.entities.window.state.text.failed.to.attach.variant2 = జోడించలేకపోయింది

journal.window.entities.window.state.text.failed.to.detach = విడదీయలేకపోయింది

journal.window.entities.window.state.text.failed.to.re.attach = మళ్ళీ జోడించలేకపోయింది

journal.window.entities.window.state.text.failed.to.save = సేవ్ చేయలేకపోయింది

journal.window.entities.window.state.text.finding.entities = ఎంటిటీలను వెతుకుతోంది…

journal.window.entities.window.state.text.loading.this.entity = ఈ ఎంటిటీ లోడ్ అవుతోంది…

journal.window.entities.window.state.text.next.page = తర్వాతి పేజీ

journal.window.entities.window.state.text.not.set = పెట్టలేదు

journal.window.entities.window.state.text.previous.page = మునుపటి పేజీ

journal.window.entities.window.state.text.suggested.match = సూచన సరిపోవడం

journal.window.entities.window.state.text.try.again = మళ్ళీ ప్రయత్నించండి.

journal.window.facets.activities.calendar.scheduled.title = అప్పుడు మీ క్యాలెండర్‌లో: { $meeting_title }

journal.window.facets.activities.copy.action.correct = అది సరైనది కాదు

journal.window.facets.activities.copy.action.edit = సవరించండి

journal.window.facets.activities.copy.action.hide = దాచండి

journal.window.facets.activities.copy.action.separate = వేరు చేయండి

journal.window.facets.activities.copy.action.separate.yes = దాన్ని వేరు చేయండి

journal.window.facets.activities.copy.action.unhide = మళ్ళీ చూపండి

journal.window.facets.activities.copy.back = వెనుకకు

journal.window.facets.activities.copy.cancel = రద్దు చేయండి

journal.window.facets.activities.copy.correct.title = అది సరైనది కాదు

journal.window.facets.activities.copy.correct.what = నిజంగా ఏమి జరిగింది

journal.window.facets.activities.copy.correct.who = ఎవరు చెప్పారు

journal.window.facets.activities.copy.corrected.by.you = మీరు సరిచేశారు

journal.window.facets.activities.copy.cue.edited = మీరు సవరించారు

journal.window.facets.activities.copy.empty.facet.show.all = అన్ని ఫాసెట్‌లు చూపండి

journal.window.facets.activities.copy.empty.today.heading = ఈ రోజు ఇంకా ఏమీ లేదు

journal.window.facets.activities.copy.evidence.open = ట్రాన్స్క్రిప్ట్‌లలో తెరవండి ›

journal.window.facets.activities.copy.evidence.open.narrow = తెరవండి ›

journal.window.facets.activities.copy.heading.accounts = దేనివల్ల నమోదు అయింది

journal.window.facets.activities.copy.heading.description = వివరణ

journal.window.facets.activities.copy.heading.history = చరిత్ర

journal.window.facets.activities.copy.heading.summary = సారాంశం

journal.window.facets.activities.copy.hidden.outline = దాచబడింది

journal.window.facets.activities.copy.history.remove = తీసివేయండి

journal.window.facets.activities.copy.history.removed = తీసివేయబడింది

journal.window.facets.activities.copy.history.you.hid = మీరు దీన్ని దాచారు

journal.window.facets.activities.copy.history.you.unhid = మీరు దీన్ని మళ్ళీ చూపారు

journal.window.facets.activities.copy.home.today.link = ఈ రోజు కార్యకలాపాలన్నీ →

journal.window.facets.activities.copy.label.description = వివరణ

journal.window.facets.activities.copy.label.title = శీర్షిక

journal.window.facets.activities.copy.lines.loading = మాట్లాడిన పంక్తులను వెతుకుతోంది…

journal.window.facets.activities.copy.lines.only = ఈ పంక్తులు మాత్రమే

journal.window.facets.activities.copy.lines.others = ఇతర మాట్లాడిన పంక్తులు

journal.window.facets.activities.copy.load.error = కార్యకలాపాలు లోడ్ చేయలేకపోయింది

journal.window.facets.activities.copy.loading = కార్యకలాపాలు లోడ్ అవుతున్నాయి…

journal.window.facets.activities.copy.now = ఇప్పుడు

journal.window.facets.activities.copy.people.loading = మీ జర్నల్‌లోని వ్యక్తులు లోడ్ అవుతున్నారు…

journal.window.facets.activities.copy.person.search = వ్యక్తిని వెతకండి

journal.window.facets.activities.copy.reason.other = దాచబడింది

journal.window.facets.activities.copy.reason.owner = మీరు దీన్ని దాచారు

journal.window.facets.activities.copy.save = సేవ్ చేయండి

journal.window.facets.activities.copy.scope.all = అన్ని ఫాసెట్‌లు చూపిస్తోంది

journal.window.facets.activities.copy.scope.all.option = అన్ని ఫాసెట్‌లు

journal.window.facets.activities.copy.sheet.close = మూసివేయండి

journal.window.facets.activities.copy.show.hidden = దాచబడినవి చూపండి

journal.window.facets.activities.copy.source.unknown = మూలం తెలియదు

journal.window.facets.activities.copy.speaker.me = మీరు

journal.window.facets.activities.copy.speaker.person = ఒక వ్యక్తి

journal.window.facets.activities.copy.state.removed = తీసివేయబడింది

journal.window.facets.activities.copy.state.removed.by.you = మీరు తీసివేశారు

journal.window.facets.activities.copy.stop.showing.hidden = దాచబడినవి చూపడం ఆపండి

journal.window.facets.activities.copy.unhide = మళ్ళీ చూపండి

journal.window.facets.activities.copy.untimed.heading = సమయం లేకుండా

journal.window.facets.activities.copy.updating = అప్‌డేట్ అవుతోంది…

journal.window.facets.activities.empty.facet.day = ఈ రోజు { $facet_name } అనే ఫాసెట్‌లో ఏమీ లేదు

journal.window.facets.activities.evidence.duration.minutes = { $quantity ->
    [one] { $count } ని
   *[other] { $count } ని
}

journal.window.facets.activities.evidence.segments.mixed = { $quantity ->
    [one] ఒకటి కంటే ఎక్కువ మూలాల నుండి 1 సెగ్మెంట్, { $time_range }
   *[other] ఒకటి కంటే ఎక్కువ మూలాల నుండి { $count } సెగ్మెంట్‌లు, { $time_range }
}

journal.window.facets.activities.evidence.segments.named = { $quantity ->
    [one] { $stream_name } నుండి 1 సెగ్మెంట్, { $time_range }
   *[other] { $stream_name } నుండి { $count } సెగ్మెంట్‌లు, { $time_range }
}

journal.window.facets.activities.evidence.segments.unknown = { $quantity ->
    [one] తెలియని మూలం నుండి 1 సెగ్మెంట్, { $time_range }
   *[other] తెలియని మూలం నుండి { $count } సెగ్మెంట్‌లు, { $time_range }
}

journal.window.facets.activities.evidence.show.all.count = { $quantity ->
    [one] { $count } సెగ్మెంట్ చూపండి
   *[other] { $count } సెగ్మెంట్‌లు అన్నీ చూపండి
}

journal.window.facets.activities.hidden.footer.count = { $quantity ->
    [one] ఈ రోజు { $count } దాచబడింది
   *[other] ఈ రోజు { $count } దాచబడ్డాయి
}

journal.window.facets.activities.hidden.showing.count = { $quantity ->
    [one] { $count } దాచబడింది చూపిస్తోంది
   *[other] { $count } దాచబడ్డాయి చూపిస్తోంది
}

journal.window.facets.activities.history.changed.fields = { $field_group ->
   *[title] మీరు శీర్షికను మార్చారు
    [description] మీరు వివరణను మార్చారు
    [details] మీరు దాని గురించి మీ వృత్తాంతాన్ని మార్చారు
    [title-description] మీరు శీర్షికను, వివరణను మార్చారు
    [title-details] మీరు శీర్షికను, దాని గురించి మీ వృత్తాంతాన్ని మార్చారు
    [description-details] మీరు వివరణను, దాని గురించి మీ వృత్తాంతాన్ని మార్చారు
    [title-description-details] మీరు శీర్షికను, వివరణను, దాని గురించి మీ వృత్తాంతాన్ని మార్చారు
}

journal.window.facets.activities.history.changed.unclassified = మీరు మార్చారు{ $slot1 }

journal.window.facets.activities.history.corrected.lines = { $quantity ->
    [one] 1 పంక్తిని ఎవరు అన్నారో మీరు సరిచేశారు
   *[other] { $count } పంక్తులను ఎవరు అన్నారో మీరు సరిచేశారు
}

journal.window.facets.activities.history.found.stream = { $stream_name } నుండి వచ్చిన సెగ్మెంట్‌లలో దొరికింది

journal.window.facets.activities.history.remove.description = తీసివేయండి: { $recorded_change }

journal.window.facets.activities.history.suggested.agent = { $agent } సూచించింది

journal.window.facets.activities.line.correction.help = మీ సమాధానం మీరు టిక్ చేసిన పంక్తులకే, గొంతులకు కాదు. ఒక గొంతును మీ జర్నల్‌కు నేర్పడానికి, లేదా మీ జర్నల్‌కు తెలిసిన గొంతులను చక్కబెట్టడానికి, { $speakers_start }మాట్లాడిన వారు{ $speakers_end } వద్దకు వెళ్ళండి.

journal.window.facets.activities.line.drawer.count = { $quantity ->
    [one] 1 పంక్తి
   *[other] { $count } పంక్తులు
}

journal.window.facets.activities.line.play.time = { $time } వద్ద పంక్తిని ప్లే చేయండి

journal.window.facets.activities.line.question.selected = { $quantity ->
    [one] మీరు టిక్ చేసిన పంక్తిని ఎవరు అన్నారు?
   *[other] మీరు టిక్ చేసిన { $count } పంక్తులను ఎవరు అన్నారు?
}

journal.window.facets.activities.months.short.apr = ఏప్రి

journal.window.facets.activities.months.short.aug = ఆగ

journal.window.facets.activities.months.short.dec = డిసెం

journal.window.facets.activities.months.short.feb = ఫిబ్ర

journal.window.facets.activities.months.short.jan = జన

journal.window.facets.activities.months.short.jul = జులై

journal.window.facets.activities.months.short.jun = జూన్

journal.window.facets.activities.months.short.mar = మార్చి

journal.window.facets.activities.months.short.may = మే

journal.window.facets.activities.months.short.nov = నవం

journal.window.facets.activities.months.short.oct = అక్టో

journal.window.facets.activities.months.short.sep = సెప్టెం

journal.window.facets.activities.people.recorded.more = { $quantity ->
    [one] { $first_person }, { $second_person } మరియు మరో 1 వ్యక్తితో
   *[other] { $first_person }, { $second_person } మరియు మరో { $others } వ్యక్తులతో
}

journal.window.facets.activities.people.recorded.one = { $person } తో

journal.window.facets.activities.people.recorded.two = { $first_person } మరియు { $second_person } తో

journal.window.facets.activities.place.recorded.then = { $device_name } అప్పుడు { $place } వద్ద ఉంది

journal.window.facets.activities.response.detail.disallowed.fields = ప్యాచ్‌లో అనుమతి లేని ఫీల్డ్‌లు ఉన్నాయి: { $field_list }

journal.window.facets.activities.response.detail.previous.answer.unreadable = ఈ సవరణ భర్తీ చేసిన సమాధానం చదవలేం: { $read_error }

journal.window.facets.activities.response.detail.segment.outside.activity = { $segment_path } ఈ కార్యకలాపానికి చెందిన సెగ్మెంట్ కాదు

journal.window.facets.activities.response.error.detail.activities.couldn.t.be.updated.right = కార్యకలాపాలను ఇప్పుడు అప్‌డేట్ చేయలేకపోయింది, ఎందుకంటే అవి బిజీగా ఉన్నాయి. కాసేపట్లో మళ్ళీ ప్రయత్నించండి.

journal.window.facets.activities.response.error.detail.day.must.not.be.after.today = రోజు ఈ రోజు కంటే తర్వాత ఉండకూడదు

journal.window.facets.activities.response.error.detail.day.not.found = రోజు దొరకలేదు

journal.window.facets.activities.response.error.detail.each.line.names.its.stream.segment = ప్రతి పంక్తి వీటిని పేర్కొంటుంది: ప్రవాహం, సెగ్మెంట్, మూలం, sentence_id

journal.window.facets.activities.response.error.detail.facet.must.not.be.empty = ఫాసెట్ ఖాళీగా ఉండకూడదు

journal.window.facets.activities.response.error.detail.invalid.month.format.expected.yyyymm = చెల్లని నెల ఆకృతి, { $slot1 } అయి ఉండాలి

journal.window.facets.activities.response.error.detail.lines.must.be.a.list.of = పంక్తులు పంక్తుల జాబితా అయి ఉండాలి

journal.window.facets.activities.response.error.detail.only.a.record.in.a.group = రెండు లేదా అంతకంటే ఎక్కువ ఉన్న సమూహంలోని నమోదును మాత్రమే వేరు చేయవచ్చు

journal.window.facets.activities.response.error.detail.patch.must.be.a.non.empty = ప్యాచ్ ఖాళీ కాని ఆబ్జెక్ట్ అయి ఉండాలి

journal.window.facets.activities.response.error.detail.patch.must.not.be.empty = ప్యాచ్ ఖాళీగా ఉండకూడదు

journal.window.facets.activities.response.error.detail.playback.must.be.true.or.false = ప్లేబ్యాక్ true లేదా false అయి ఉండాలి

journal.window.facets.activities.response.error.detail.position.timestamp.and.actor.name.the = స్థానం, టైమ్‌స్టాంప్, యాక్టర్ తీసివేయాల్సిన సవరణను పేర్కొంటాయి

journal.window.facets.activities.response.error.detail.removing.an.edit.needs.the.owner = ఒక సవరణను తీసివేయడానికి మీరు కావాలి

journal.window.facets.activities.response.error.detail.saying.who.said.a.line.needs = ఒక పంక్తిని ఎవరు అన్నారో చెప్పడానికి మీరు కావాలి

journal.window.facets.activities.response.error.detail.separating.a.group.needs.the.owner = ఒక సమూహాన్ని వేరు చేయడానికి మీరు కావాలి

journal.window.facets.activities.response.error.detail.source.must.be.user = మూలం { $slot1 } అయి ఉండాలి

journal.window.facets.activities.response.error.detail.speaker.is.kind.owner.kind.person = మాట్లాడిన వారు { $slot1 }, { $slot2 } లేదా { $slot3 }

journal.window.facets.activities.response.error.detail.title.must.not.be.empty = శీర్షిక ఖాళీగా ఉండకూడదు

journal.window.facets.activities.response.error.message.activities.couldn.t.be.updated.right = కార్యకలాపాలను ఇప్పుడు అప్‌డేట్ చేయలేకపోయింది, ఎందుకంటే అవి బిజీగా ఉన్నాయి. కాసేపట్లో మళ్ళీ ప్రయత్నించండి.

journal.window.facets.activities.response.error.message.only.you.can.remove.a.correction = ఒక దిద్దుబాటును మీరు మాత్రమే తీసివేయగలరు.

journal.window.facets.activities.response.error.message.only.you.can.say.who.said = ఈ పంక్తులను ఎవరు అన్నారో మీరు మాత్రమే చెప్పగలరు.

journal.window.facets.activities.response.error.message.only.you.can.separate.a.group = ఒక సమూహాన్ని మీరు మాత్రమే వేరు చేయగలరు.

journal.window.facets.activities.response.error.message.that.activity.couldn.t.be.created = ఆ కార్యకలాపాన్ని సృష్టించలేకపోయింది, ఎందుకంటే అది ఇప్పటికే ఉంది.

journal.window.facets.activities.response.error.message.that.activity.isn.t.in.the = ఆ కార్యకలాపం ఆ ఫాసెట్‌లో లేదు.

journal.window.facets.activities.response.error.message.that.activity.setting.couldn.t.be = ఆ కార్యకలాపం సెట్టింగ్‌ను వాడలేకపోయింది.

journal.window.facets.activities.response.error.message.that.day.couldn.t.be.used = ఆ రోజును వాడలేకపోయింది.

journal.window.facets.activities.response.error.message.that.edit.couldn.t.be.removed = ఆ సవరణను తీసివేయలేకపోయింది.

journal.window.facets.activities.response.error.message.that.month.couldn.t.be.used = ఆ నెలను వాడలేకపోయింది.

journal.window.facets.activities.response.error.message.that.request.didn.t.finish = ఆ అభ్యర్థన పూర్తి కాలేదు.

journal.window.facets.activities.response.error.message.this.isn.t.grouped.with.anything = ఇది ఏ సమూహంలోనూ లేదు.

journal.window.facets.activities.response.error.message.this.record.is.being.moved.you = ఈ నమోదు తరలుతోంది. అది పూర్తయ్యాక మీరు దాన్ని సవరించవచ్చు.

journal.window.facets.activities.response.error.message.who.said.those.lines.couldn.t = ఆ పంక్తులను ఎవరు అన్నారో సేవ్ చేయలేకపోయింది.

journal.window.facets.activities.scope.selected.facet = { $facet_name } చూపిస్తోంది

journal.window.facets.activities.timeline.item.hidden = { $time_range }, { $facet_name }, దాచబడింది: { $activity_title }

journal.window.facets.activities.timeline.item.hidden.source = { $time_range }, { $facet_name }, { $stream_name }, దాచబడింది: { $activity_title }

journal.window.facets.activities.timeline.item.visible = { $time_range }, { $facet_name }: { $activity_title }

journal.window.facets.activities.timeline.item.visible.source = { $time_range }, { $facet_name }, { $stream_name }: { $activity_title }

journal.window.facets.activities.window.explanation.a.line.shows.the.wrong.person = ఒక పంక్తి తప్పు వ్యక్తిని చూపిస్తోంది

journal.window.facets.activities.window.explanation.activities.appear.here.a.little.while = కార్యకలాపాలు అయిపోయిన కాసేపటి తర్వాత ఇక్కడ కనిపిస్తాయి, మరింత వచ్చేకొద్దీ ఇంకా పెరగవచ్చు.

journal.window.facets.activities.window.explanation.added.with.the.solstone.command = solstone కమాండ్‌తో చేర్చబడింది

journal.window.facets.activities.window.explanation.as.the.day.goes.on.activities = రోజు సాగేకొద్దీ, మీరు పంచుకునేదాని నుండి కార్యకలాపాలు ఇక్కడ కనిపిస్తాయి.

journal.window.facets.activities.window.explanation.can.t.be.read.right.now = ఇప్పుడు చదవలేం

journal.window.facets.activities.window.explanation.changed.with.the.solstone.command = solstone కమాండ్‌తో మార్చబడింది

journal.window.facets.activities.window.explanation.couldn.t.load.the.people.in = మీ జర్నల్‌లోని వ్యక్తులను లోడ్ చేయలేకపోయింది.

journal.window.facets.activities.window.explanation.couldn.t.remove.that.try.again = దాన్ని తీసివేయలేకపోయింది. కాసేపట్లో మళ్ళీ ప్రయత్నించండి.

journal.window.facets.activities.window.explanation.couldn.t.save.that.try.again = దాన్ని సేవ్ చేయలేకపోయింది. కాసేపట్లో మళ్ళీ ప్రయత్నించండి.

journal.window.facets.activities.window.explanation.couldn.t.save.your.changes.are = సేవ్ చేయలేకపోయింది. మీ మార్పులు ఇక్కడే ఉన్నాయి, కాబట్టి మళ్ళీ ప్రయత్నించవచ్చు.

journal.window.facets.activities.window.explanation.found.in.segments.from.an.unknown = తెలియని మూలం నుండి వచ్చిన సెగ్మెంట్‌లలో దొరికింది

journal.window.facets.activities.window.explanation.found.in.segments.from.more.than = ఒకటి కంటే ఎక్కువ మూలాల నుండి వచ్చిన సెగ్మెంట్‌లలో దొరికింది

journal.window.facets.activities.window.explanation.grew.as.more.of.it.came = దానిలో మరింత వచ్చేకొద్దీ పెరిగింది

journal.window.facets.activities.window.explanation.hidden.it.s.still.in.your = దాచబడింది. అది ఇంకా మీ జర్నల్‌లో ఉంది.

journal.window.facets.activities.window.explanation.hidden.with.the.solstone.command = solstone కమాండ్‌తో దాచబడింది

journal.window.facets.activities.window.explanation.it.won.t.join.this.group = ఇది ఈ సమూహంలో మళ్ళీ చేరదు. ఏమీ తొలగించబడదు.

journal.window.facets.activities.window.explanation.lines.your.journal.credits.to.you = మీ జర్నల్ మీవిగా పరిగణించే పంక్తులు

journal.window.facets.activities.window.explanation.more.than.one.source = ఒకటి కంటే ఎక్కువ మూలాలు

journal.window.facets.activities.window.explanation.no.activities.on.this.day = ఈ రోజు కార్యకలాపాలు లేవు

journal.window.facets.activities.window.explanation.no.longer.in.your.journal = మీ జర్నల్‌లో ఇక లేదు

journal.window.facets.activities.window.explanation.no.one.in.your.journal.matches = మీ జర్నల్‌లో ఎవరూ దానికి సరిపోలేదు.

journal.window.facets.activities.window.explanation.on.your.calendar.then = అప్పుడు మీ క్యాలెండర్‌లో

journal.window.facets.activities.window.explanation.other.facets.have.activities.on.this = ఈ రోజు ఇతర ఫాసెట్‌లలో కార్యకలాపాలు ఉన్నాయి.

journal.window.facets.activities.window.explanation.part.of.it.removed = దానిలో కొంత తీసివేయబడింది

journal.window.facets.activities.window.explanation.pick.an.activity.to.see.what = దేనిపై ఆధారపడి ఉందో చూడటానికి ఒక కార్యకలాపాన్ని ఎంచుకోండి.

journal.window.facets.activities.window.explanation.say.it.the.way.you.d = దగ్గరి వ్యక్తికి చెప్పినట్టు చెప్పండి: ఎవరు ఏమి చేశారు, ఏమి నిర్ణయం అయింది, ఏమి ఇంకా తెరిచి ఉంది.

journal.window.facets.activities.window.explanation.seen.as.cancelled.or.declined = రద్దు అయినట్టు లేదా నిరాకరించినట్టు కనిపించింది

journal.window.facets.activities.window.explanation.some.of.those.lines.aren.t = ఆ పంక్తుల్లో కొన్ని ఈ కార్యకలాపంలో భాగం కావు.

journal.window.facets.activities.window.explanation.someone.i.don.t.know = మీకు తెలియని వ్యక్తి

journal.window.facets.activities.window.explanation.tell.your.journal.in.your.own = మీ మాటల్లో మీ జర్నల్‌కు చెప్పండి

journal.window.facets.activities.window.explanation.that.changed.since.it.was.shown = చూపించిన తర్వాత అది మారింది. ఇప్పుడు ఇలా ఉంది.

journal.window.facets.activities.window.explanation.that.person.can.t.be.chosen = ఆ వ్యక్తిని ఎంచుకోలేం. వేరొకరిని ఎంచుకోండి, లేదా మీకు తెలియని వ్యక్తిని ఎంచుకోండి.

journal.window.facets.activities.window.explanation.the.journal.adds.activities.as.it = మీరు పంచుకున్నదానిపై పని చేస్తూ జర్నల్ కార్యకలాపాలను చేరుస్తుంది.

journal.window.facets.activities.window.explanation.these.lines.can.t.take.an = ఈ పంక్తులు, వాటిని ఎవరు అన్నారనే సమాధానం తీసుకోలేవు. బదులుగా ఏమి జరిగిందో మీ జర్నల్‌కు చెప్పవచ్చు.

journal.window.facets.activities.window.explanation.these.lines.have.changed.since.they = చూపించిన తర్వాత ఈ పంక్తులు మారాయి. వెనక్కి వెళ్లి వాటిని మళ్ళీ తెరవండి.

journal.window.facets.activities.window.explanation.this.was.a.recording.or.video = ఇది ప్లే అవుతున్న రికార్డింగ్ లేదా వీడియో

journal.window.facets.activities.window.explanation.tick.the.lines.you.want.to = ముందు సరిచేయాలనుకున్న పంక్తులను టిక్ చేయండి.

journal.window.facets.activities.window.explanation.unhidden.with.the.solstone.command = solstone కమాండ్‌తో మళ్ళీ చూపబడింది

journal.window.facets.activities.window.explanation.unhide.the.day.on.the.body = శరీరం పేజీలో ఈ రోజును మళ్ళీ చూపండి

journal.window.facets.activities.window.explanation.what.it.s.based.on = దేనిపై ఆధారపడి ఉంది

journal.window.facets.activities.window.explanation.who.said.the.lines.you.ticked = మీరు టిక్ చేసిన పంక్తులను ఎవరు అన్నారు?

journal.window.facets.activities.window.explanation.yes.it.was.playing = అవును, అది ప్లే అవుతూ ఉంది

journal.window.facets.activities.window.explanation.you.hid.this.day.s.body = మీరు ఈ రోజు శరీర డేటాను దాచారు

journal.window.facets.activities.window.explanation.you.said.this.was.a.recording = ఇది ప్లే అవుతున్న రికార్డింగ్ లేదా వీడియో అని మీరు చెప్పారు

journal.window.facets.activities.window.explanation.you.told.your.journal.what.happened = ఏమి జరిగిందో మీరు మీ జర్నల్‌కు చెప్పారు

journal.window.facets.activities.window.explanation.your.account.of.it = దాని గురించి మీ వృత్తాంతం

journal.window.facets.activities.window.explanation.your.earlier.answers.about.who.spoke = ఇక్కడ ఎవరు మాట్లాడారో మీ ముందు సమాధానాలు చదవలేకపోయింది, అందుకే ఇది సేవ్ కాలేదు.

journal.window.facets.activities.window.explanation.your.journal.doesn.t.know.which = మీ జర్నల్‌కు మీరు ఏ వ్యక్తో ఇంకా తెలియదు, అందుకే పంక్తులను మీవిగా గుర్తు పెట్టలేదు.

journal.window.facets.activities.window.explanation.your.journal.will.think.this.through = మీరు చూసిన లేదా విన్న దానిగా, వేరే ఎవరూ పాల్గొనకుండా, మీ జర్నల్ దీన్ని మళ్ళీ ఆలోచిస్తుంది.

journal.window.facets.awareness.response.detail.multiple.import.answers = { $slot1 } లో ఒకటే ఇవ్వండి; వచ్చింది { $answer_list }

journal.window.facets.awareness.response.detail.unknown.section = '{ $section_name }' అనే అవేర్‌నెస్ విభాగం లేదు

journal.window.facets.awareness.response.detail.unreadable.settings = { $settings_path } వద్ద ఉన్న మీ సెట్టింగ్‌ల ఫైల్ చదవలేకపోయింది. మీ సెట్టింగ్‌లు మారలేదు. ఫైల్‌ను బాగుచేయండి, లేదా బ్యాకప్ నుండి { $slot3 } రీస్టోర్ చేయండి, తర్వాత మళ్ళీ ప్రయత్నించండి.

journal.window.facets.awareness.response.error.detail.kind.is.required = రకం అవసరం

journal.window.facets.awareness.response.error.detail.no.request.body = అభ్యర్థన బాడీ లేదు

journal.window.facets.awareness.response.error.detail.request.body.must.be.a.json = అభ్యర్థన బాడీ { $slot1 } ఆబ్జెక్ట్ అయి ఉండాలి

journal.window.facets.awareness.response.error.message.a.required.field.is.missing = అవసరమైన ఫీల్డ్ లేదు.

journal.window.facets.awareness.response.error.message.one.of.those.values.couldn.t = ఆ విలువల్లో ఒకదాన్ని వాడలేకపోయింది.

journal.window.facets.awareness.response.error.message.that.json.request.couldn.t.be = ఆ { $slot1 } అభ్యర్థన చదవలేకపోయింది.

journal.window.facets.awareness.response.error.message.that.part.of.your.journal.couldn = మీ జర్నల్‌లో ఆ భాగం దొరకలేదు.

journal.window.facets.awareness.response.error.message.that.request.didn.t.finish = ఆ అభ్యర్థన పూర్తి కాలేదు.

journal.window.facets.awareness.response.error.message.that.request.had.no.data.in = ఆ అభ్యర్థనలో డేటా లేదు.

journal.window.facets.awareness.response.error.message.the.awareness.operation.is.busy = అవేర్‌నెస్ ఆపరేషన్ బిజీగా ఉంది.

journal.window.facets.awareness.response.error.message.your.settings.couldn.t.be.read = మీ సెట్టింగ్‌లను చదవలేకపోయింది.

journal.window.facets.awareness.response.imports.busy.detail = ఇంపోర్ట్‌లు బిజీగా ఉన్నాయి; మళ్ళీ ప్రయత్నించండి

journal.window.facets.awareness.response.one.import.answer = { $slot1 } లో ఒకటే ఇవ్వండి

journal.window.facets.curation.ambiguity.choose.name = { $candidate_name } ఎంచుకోండి

journal.window.facets.curation.ambiguity.member.occurrences = { $quantity ->
    [one] { $count } సంఘటన
   *[other] { $count } సంఘటనలు
}

journal.window.facets.curation.ambiguity.occurrences = { $quantity ->
    [one] { $count } సంఘటన
   *[other] { $count } సంఘటనలు
}

journal.window.facets.curation.ambiguity.question = "{ $ambiguous_name }" తో ఏ ఎంట్రీ సరిపోతుంది?

journal.window.facets.curation.button.label.try.again = మళ్ళీ ప్రయత్నించండి

journal.window.facets.curation.disclosure.label.individual.questions.and.sources = విడివిడి ప్రశ్నలు, మూలాలు

journal.window.facets.curation.disclosure.label.source.evidence = మూల ఆధారం

journal.window.facets.curation.disclosure.label.source.identifiers = మూల ఐడెంటిఫయర్‌లు

journal.window.facets.curation.disclosure.label.technical.details = సాంకేతిక వివరాలు

journal.window.facets.curation.evidence.drawer.title = ఆధారం

journal.window.facets.curation.evidence.showing.partial = { $shown_quantity ->
    [one] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $shown } చూపిస్తోంది
   *[other] { $total_quantity ->
        [one] { $total }
       *[other] { $total }
    } లో { $shown } చూపిస్తోంది
}

journal.window.facets.curation.evidence.total.pieces = { $quantity ->
    [one] 1 ముక్క
   *[other] { $count } ముక్కలు
}

journal.window.facets.curation.facet.additional.names = ఇలా కూడా కనిపించింది: { $alias_list }.

journal.window.facets.curation.facet.create.question = జర్నల్ కార్యకలాపం మీ ఫాసెట్‌లకు సరిగా ఇమడడం లేదు. "{ $facet_name }" ఫాసెట్ సృష్టించాలా?

journal.window.facets.curation.list.items = { $preceding_items } మరియు { $last_item }

journal.window.facets.curation.placeholder.this.is.a.speaker.label.not = ఇది మాట్లాడిన వారి లేబుల్, పేరు కాదు

journal.window.facets.curation.preview.activities.count = { $quantity ->
    [one] కార్యకలాప నమోదులు మార్చబడ్డాయి: { $count }
   *[other] కార్యకలాప నమోదులు మార్చబడ్డాయి: { $count }
}

journal.window.facets.curation.preview.aliases = అలియాస్‌లు చేర్చబడ్డాయి: { $aliases }

journal.window.facets.curation.preview.emails.count = { $quantity ->
    [one] ఈమెయిల్‌లు చేర్చబడ్డాయి: { $count }
   *[other] ఈమెయిల్‌లు చేర్చబడ్డాయి: { $count }
}

journal.window.facets.curation.preview.facet.links = { $moved_quantity ->
    [one] ఫాసెట్ లింక్‌లు: { $moved } తరలించబడింది, { $merged_quantity ->
        [one] { $merged } జత అయింది
       *[other] { $merged } జత అయ్యాయి
    }
   *[other] ఫాసెట్ లింక్‌లు: { $moved } తరలించబడ్డాయి, { $merged_quantity ->
        [one] { $merged } జత అయింది
       *[other] { $merged } జత అయ్యాయి
    }
}

journal.window.facets.curation.preview.notes.count = { $quantity ->
    [one] జత అవుతున్న పేరు కింద నోట్‌లు: { $count }
   *[other] జత అవుతున్న పేరు కింద నోట్‌లు: { $count }
}

journal.window.facets.curation.preview.observations.count = { $quantity ->
    [one] నోట్‌లు తరలించబడ్డాయి: { $count }
   *[other] నోట్‌లు తరలించబడ్డాయి: { $count }
}

journal.window.facets.curation.preview.speaker.labels.count = { $quantity ->
    [one] మాట్లాడిన వారి లేబుల్‌లు మార్చబడ్డాయి: { $count }
   *[other] మాట్లాడిన వారి లేబుల్‌లు మార్చబడ్డాయి: { $count }
}

journal.window.facets.curation.preview.speaker.segments = { $labels_quantity ->
    [one] మాట్లాడిన వారి లేబుల్‌లు అప్‌డేట్ అయ్యాయి: { $labels } లేబుల్, { $corrections_quantity ->
        [one] { $corrections } దిద్దుబాటు
       *[other] { $corrections } దిద్దుబాట్లు
    }
   *[other] మాట్లాడిన వారి లేబుల్‌లు అప్‌డేట్ అయ్యాయి: { $labels } లేబుల్‌లు, { $corrections_quantity ->
        [one] { $corrections } దిద్దుబాటు
       *[other] { $corrections } దిద్దుబాట్లు
    }
}

journal.window.facets.curation.preview.voice.samples = { $added_quantity ->
    [one] గొంతు నమూనాలు తరలించబడ్డాయి: { $added } చేర్చబడింది, { $total_quantity ->
        [one] { $total } మొత్తం
       *[other] { $total } మొత్తం
    }
   *[other] గొంతు నమూనాలు తరలించబడ్డాయి: { $added } చేర్చబడ్డాయి, { $total_quantity ->
        [one] { $total } మొత్తం
       *[other] { $total } మొత్తం
    }
}

journal.window.facets.curation.response.batch.candidate.identity.missing = అభ్యర్థిలో facet, source_slug, లేదా target_slug లేదు

journal.window.facets.curation.response.candidate.absent = అభ్యర్థి దొరకలేదు

journal.window.facets.curation.response.candidate.accept.status = స్థితి{ $slot1 }{ $status } ఉన్న అభ్యర్థిని అంగీకరించలేం

journal.window.facets.curation.response.candidate.dismiss.status = స్థితి{ $slot1 }{ $status } ఉన్న అభ్యర్థిని వదిలివేయలేం

journal.window.facets.curation.response.candidate.kept.name.missing = "{ $name }" ఇప్పటికే "{ $kept }" లోకి జత అయింది, దాని పేరు ఉంచకుండా. ఎంటిటీల పేజీలో "{ $kept }" ప్రత్యామ్నాయ పేర్లలో "{ $name }" చేర్చండి, తర్వాత మళ్ళీ అంగీకరించండి.

journal.window.facets.curation.response.candidate.preview.status = స్థితి{ $slot1 }{ $status } ఉన్న అభ్యర్థిని ప్రివ్యూ చూడలేం

journal.window.facets.curation.response.copy.action.failed = అది జరగలేదు. మళ్ళీ ప్రయత్నించండి.

journal.window.facets.curation.response.copy.ambiguity.body = "{ $slot1 }" తో ఏ ఎంట్రీ సరిపోతుంది?

journal.window.facets.curation.response.copy.ambiguity.choose.action = { $slot1 } ఎంచుకోండి

journal.window.facets.curation.response.copy.ambiguity.dismiss.action = వీటిలో ఏదీ కాదు

journal.window.facets.curation.response.copy.ambiguity.origin.label = కనబడిన చోటు

journal.window.facets.curation.response.copy.empty.state = సమీక్షించడానికి ఏమీ లేదు. సూచించడానికి ఇంకా కొత్త నిర్మాణం లేదు.

journal.window.facets.curation.response.copy.entity.batch.confirm.dismiss.action = అన్నిటినీ వేరుగా ఉంచండి

journal.window.facets.curation.response.copy.entity.batch.confirm.merge.action = అన్నిటినీ జత చేయండి

journal.window.facets.curation.response.copy.entity.batch.dismiss.action = ఎంచుకున్నవాటిని వేరుగా ఉంచండి

journal.window.facets.curation.response.copy.entity.batch.dismiss.lead = ఈ జతలు వేరుగా ఉంచబడతాయి:

journal.window.facets.curation.response.copy.entity.batch.dismiss.summary = { $slot2 } లో { $slot1 } వేరుగా ఉంచబడ్డాయి.

journal.window.facets.curation.response.copy.entity.batch.failed.note = { $slot1 } కింద ఇంకా శ్రద్ధ కావాలి.

journal.window.facets.curation.response.copy.entity.batch.merge.action = ఎంచుకున్నవాటిని జత చేయండి

journal.window.facets.curation.response.copy.entity.batch.merge.lead = ఈ జతలు జత అవుతాయి. జతలను అన్‌డూ చేయలేం:

journal.window.facets.curation.response.copy.entity.batch.merge.summary = { $slot2 } లో { $slot1 } జత అయ్యాయి.

journal.window.facets.curation.response.copy.entity.body = "{ $slot1 }" మరియు "{ $slot2 }" ఒకే ఎంటిటీలా కనిపిస్తున్నాయి. వాటిని జత చేయాలా?

journal.window.facets.curation.response.copy.entity.cancel.action = రద్దు చేయండి

journal.window.facets.curation.response.copy.entity.confirm.action = జతను నిర్ధారించండి

journal.window.facets.curation.response.copy.entity.dismiss.action = వేరుగా ఉంచండి

journal.window.facets.curation.response.copy.entity.done.for.now.action = ఇప్పటికి పూర్తి

journal.window.facets.curation.response.copy.entity.facets.label = ({ $slot1 } లో)

journal.window.facets.curation.response.copy.entity.group.lede = ఈ { $slot1 } జతలు ఒకేలా కనిపిస్తున్నాయి. వాటిని జత చేయడం మీ పేర్లను సరిగా ఉంచుతుంది.

journal.window.facets.curation.response.copy.entity.merge.action = జత చేయండి

journal.window.facets.curation.response.copy.entity.preview.empty = ఈ జతకు జర్నల్ మార్పులు అవసరం లేదు.

journal.window.facets.curation.response.copy.entity.preview.errors = కొన్ని సెగ్మెంట్ అప్‌డేట్‌లకు శ్రద్ధ కావచ్చు.

journal.window.facets.curation.response.copy.entity.preview.lead = జత చేయడానికి ముందు, ఏమి మారుతుందో ఇది. జతను అన్‌డూ చేయలేం.

journal.window.facets.curation.response.copy.entity.progress.label = { $slot1 } సమీక్ష అయింది · { $slot2 } మిగిలి ఉంది

journal.window.facets.curation.response.copy.entity.select.all.action = అన్నీ ఎంచుకోండి

journal.window.facets.curation.response.copy.entity.show.rest.action = మిగిలినవి చూపండి ({ $slot1 })

journal.window.facets.curation.response.copy.facet.body = జర్నల్ కార్యకలాపం మీ ఫాసెట్‌లకు సరిగా ఇమడడం లేదు. "{ $slot1 }" ఫాసెట్ సృష్టించాలా?

journal.window.facets.curation.response.copy.facet.create.action = ఫాసెట్ సృష్టించండి

journal.window.facets.curation.response.copy.facet.dismiss.action = ఇప్పుడు కాదు

journal.window.facets.curation.response.copy.heading = సూచనలు

journal.window.facets.curation.response.copy.preview.activities = కార్యకలాప నమోదులు మార్చబడ్డాయి

journal.window.facets.curation.response.copy.preview.akas.label = అలియాస్‌లు చేర్చబడ్డాయి

journal.window.facets.curation.response.copy.preview.emails.label = ఈమెయిల్‌లు చేర్చబడ్డాయి

journal.window.facets.curation.response.copy.preview.facets.label = ఫాసెట్ లింక్‌లు

journal.window.facets.curation.response.copy.preview.labels = మాట్లాడిన వారి లేబుల్‌లు మార్చబడ్డాయి

journal.window.facets.curation.response.copy.preview.notes.source = జత అవుతున్న పేరు కింద నోట్‌లు

journal.window.facets.curation.response.copy.preview.observations.label = నోట్‌లు తరలించబడ్డాయి

journal.window.facets.curation.response.copy.preview.partial = ఈ జత ఫాసెట్ లింక్‌లను, ఫోటోలను కూడా కలుపుతుంది.

journal.window.facets.curation.response.copy.preview.segments.label = మాట్లాడిన వారి లేబుల్‌లు అప్‌డేట్ అయ్యాయి

journal.window.facets.curation.response.copy.preview.voiceprints.label = గొంతు నమూనాలు తరలించబడ్డాయి

journal.window.facets.curation.response.copy.repair.required = { $slot1 } { $slot2 }

journal.window.facets.curation.response.copy.review.restore.action = సమీక్షకు రీస్టోర్ చేయండి

journal.window.facets.curation.response.copy.speaker.body = "{ $slot1 }" మరియు "{ $slot2 }" ఒకే మాట్లాడిన వారు కావచ్చు. వాటిని జత చేయాలా?

journal.window.facets.curation.response.copy.speaker.candidate.pair.body = మీ జర్నల్‌లో రెండు గొంతులు ఒకేలా వినిపిస్తున్నాయి. వాటిని జత చేయాలా?

journal.window.facets.curation.response.copy.speaker.candidate.pair.dismiss.action = వేరుగా ఉంచండి

journal.window.facets.curation.response.copy.speaker.candidate.pair.intervals.label = విరామాలు

journal.window.facets.curation.response.copy.speaker.candidate.pair.merge.action = అభ్యర్థులను జత చేయండి

journal.window.facets.curation.response.copy.speaker.candidate.pair.similarity.label = కొసైన్

journal.window.facets.curation.response.copy.speaker.candidate.pair.source.label = అభ్యర్థి A

journal.window.facets.curation.response.copy.speaker.candidate.pair.target.label = అభ్యర్థి B

journal.window.facets.curation.response.copy.speaker.dismiss.action = వేరుగా ఉంచండి

journal.window.facets.curation.response.copy.speaker.merge.action = జతను సమీక్షించండి

journal.window.facets.curation.response.detail.missing.field = { $field_name } లేదు

journal.window.facets.curation.response.detail.unknown.group = తెలియని క్యూరేషన్ సమూహం: { $group }

journal.window.facets.curation.response.detail.unreadable.settings = { $settings_path } వద్ద ఉన్న మీ సెట్టింగ్‌ల ఫైల్ చదవలేకపోయింది. మీ సెట్టింగ్‌లు మారలేదు. ఫైల్‌ను బాగుచేయండి, లేదా బ్యాకప్ నుండి { $slot3 } రీస్టోర్ చేయండి, తర్వాత మళ్ళీ ప్రయత్నించండి.

journal.window.facets.curation.response.entity.busy.detail = ఎంటిటీ జత అభ్యర్థులు బిజీగా ఉన్నాయి; మళ్ళీ ప్రయత్నించండి

journal.window.facets.curation.response.error.detail.key.does.not.match.anchor.a = కీ { $slot1 } తో సరిపోలేదు

journal.window.facets.curation.response.error.detail.key.does.not.match.source.id = కీ { $slot1 } తో సరిపోలేదు

journal.window.facets.curation.response.error.message.a.required.field.is.missing = అవసరమైన ఫీల్డ్ లేదు.

journal.window.facets.curation.response.error.message.one.of.those.values.couldn.t = ఆ విలువల్లో ఒకదాన్ని వాడలేకపోయింది.

journal.window.facets.curation.response.error.message.that.entity.operation.couldn.t.be = ఆ ఎంటిటీ ఆపరేషన్ పూర్తి చేయలేకపోయింది.

journal.window.facets.curation.response.error.message.that.list.isn.t.one.curation = ఆ జాబితా క్యూరేషన్ తిప్పే జాబితా కాదు.

journal.window.facets.curation.response.error.message.the.entity.operation.is.busy = ఎంటిటీ ఆపరేషన్ బిజీగా ఉంది.

journal.window.facets.curation.response.error.message.your.settings.couldn.t.be.read = మీ సెట్టింగ్‌లను చదవలేకపోయింది.

journal.window.facets.curation.response.pair.already.confirmed = నిర్ధారించిన రెండు అభ్యర్థులను జత చేయలేం

journal.window.facets.curation.response.pair.already.merged = అభ్యర్థుల జత ఇప్పటికే జత అయింది

journal.window.facets.curation.response.pair.anchor.absent = అభ్యర్థి యాంకర్ దొరకలేదు

journal.window.facets.curation.response.pair.below.threshold = అభ్యర్థుల జత సమీక్ష పరిమితి కంటే తక్కువ

journal.window.facets.curation.response.pair.rejected = నిరాకరించిన అభ్యర్థిని జత చేయలేం

journal.window.facets.curation.response.speaker.busy.detail = మాట్లాడిన వారి సూచనలు బిజీగా ఉన్నాయి; మళ్ళీ ప్రయత్నించండి

journal.window.facets.curation.review.total = { $quantity ->
    [one] సమీక్షించాల్సిన { $count } సూచన. మొదలుపెట్టడానికి ఒక సమూహాన్ని ఎంచుకోండి.
   *[other] సమీక్షించాల్సిన { $count } సూచనలు. మొదలుపెట్టడానికి ఒక సమూహాన్ని ఎంచుకోండి.
}

journal.window.facets.curation.speaker.merge.question = "{ $source_name }" మరియు "{ $target_name }" ఒకే మాట్లాడిన వారు కావచ్చు. వారిని జత చేయాలా?

journal.window.facets.curation.status.text.journal = జర్నల్

journal.window.facets.curation.status.text.please.select.at.least.one.question = పరిష్కరించడానికి కనీసం ఒక ప్రశ్నను ఎంచుకోండి.

journal.window.facets.curation.status.text.restore.to.review = సమీక్షకు రీస్టోర్ చేయండి

journal.window.facets.curation.suggestion.display.text.another.change.is.in.progress.try = మరో మార్పు జరుగుతోంది. కొద్ది సేపట్లో మళ్ళీ ప్రయత్నించండి.

journal.window.facets.curation.suggestion.display.text.automatic.review = స్వయంచాలక సమీక్ష

journal.window.facets.curation.suggestion.display.text.couldn.t.apply.that.choice.refresh = ఆ ఎంపికను అమలు చేయలేకపోయింది. సూచనలు రిఫ్రెష్ చేసి మళ్ళీ ప్రయత్నించండి.

journal.window.facets.curation.suggestion.display.text.couldn.t.save.that.change.try = ఆ మార్పును సేవ్ చేయలేకపోయింది. మళ్ళీ ప్రయత్నించండి.

journal.window.facets.curation.suggestion.display.text.facet.suggestions = ఫాసెట్ సూచనలు

journal.window.facets.curation.suggestion.display.text.names.to.clarify = స్పష్టం చేయాల్సిన పేర్లు

journal.window.facets.curation.suggestion.display.text.possible.duplicate.voices = సాధ్యమైన నకలు గొంతులు

journal.window.facets.curation.suggestion.display.text.show.more = మరిన్ని చూపండి

journal.window.facets.curation.suggestion.display.text.this.suggestion.or.one.of.its = ఈ సూచన లేదా దాని సరిపోయేవాటిలో ఒకటి ఇక అందుబాటులో లేదు. సూచనలు రిఫ్రెష్ చేయండి.

journal.window.facets.curation.suggestions.accept = అంగీకరించండి

journal.window.facets.curation.suggestions.account.attributed.agent = { $agent_name } రాసినట్టు మీ వృత్తాంతం

journal.window.facets.curation.suggestions.basis.agent.journal.shows = { $agent_name } దీన్ని మీ జర్నల్‌లో గమనించిందని చెబుతోంది

journal.window.facets.curation.suggestions.basis.agent.my.own.work = { $agent_name } దీన్ని నేరుగా తెలుసు అని చెబుతోంది

journal.window.facets.curation.suggestions.basis.agent.outside.source = { $agent_name } దీన్ని మీ జర్నల్ బయట కనుగొందని చెబుతోంది

journal.window.facets.curation.suggestions.basis.agent.owner.said = { $agent_name } మీరు దానికి చెప్పారని చెబుతోంది

journal.window.facets.curation.suggestions.cancel = రద్దు చేయండి

journal.window.facets.curation.suggestions.dismiss = వదిలివేయండి

journal.window.facets.curation.suggestions.earlier.accepted = మీరు దీన్ని అంగీకరించారు

journal.window.facets.curation.suggestions.earlier.agent = ముందటి ఏజెంట్

journal.window.facets.curation.suggestions.earlier.dismissed = మీరు దీన్ని వదిలివేశారు

journal.window.facets.curation.suggestions.earlier.in.duplicates = సాధ్యమైన నకలుల్లో

journal.window.facets.curation.suggestions.earlier.not.accepted = అంగీకరించబడలేదు

journal.window.facets.curation.suggestions.earlier.stale = ఇప్పటికే మారింది

journal.window.facets.curation.suggestions.edit = సవరించండి

journal.window.facets.curation.suggestions.edit.name.label = పేరు

journal.window.facets.curation.suggestions.filter.empty.agent = { $agent_name } నుండి ఏదీ వేచి లేదు.

journal.window.facets.curation.suggestions.filter.only.agent = { $agent_name } నుండి మాత్రమే.

journal.window.facets.curation.suggestions.group = మీ ఏజెంట్‌లు నుండి

journal.window.facets.curation.suggestions.keep.name = వాడాల్సిన పేరు

journal.window.facets.curation.suggestions.kind.name = ఒక పేరు

journal.window.facets.curation.suggestions.kind.same.person = ఒకే వ్యక్తి

journal.window.facets.curation.suggestions.kind.what.happened = నిజంగా ఏమి జరిగింది

journal.window.facets.curation.suggestions.kind.who.said.it = ఎవరు చెప్పారు

journal.window.facets.curation.suggestions.lines.assigned.named = { $quantity ->
    [one] { $speaker } ఈ పంక్తి చెప్పారు
   *[other] { $speaker } ఈ { $count } పంక్తులు చెప్పారు
}

journal.window.facets.curation.suggestions.lines.assigned.owner = { $quantity ->
    [one] { $speaker_start }మీరు{ $speaker_end } ఈ పంక్తి చెప్పారు
   *[other] { $speaker_start }మీరు{ $speaker_end } ఈ { $count } పంక్తులు చెప్పారు
}

journal.window.facets.curation.suggestions.lines.assigned.unknown = { $quantity ->
    [one] { $speaker_start }మీకు తెలియని ఎవరో{ $speaker_end } ఈ పంక్తి చెప్పారు
   *[other] { $speaker_start }మీకు తెలియని ఎవరో{ $speaker_end } ఈ { $count } పంక్తులు చెప్పారు
}

journal.window.facets.curation.suggestions.name.alias = { $former_name }, ఇలా కూడా { $new_name }

journal.window.facets.curation.suggestions.name.change = { $former_name } → { $new_name }

journal.window.facets.curation.suggestions.now = మీ జర్నల్ చెబుతోంది

journal.window.facets.curation.suggestions.now.people = మీ జర్నల్‌లో ఉంది

journal.window.facets.curation.suggestions.ok = సరే

journal.window.facets.curation.suggestions.open = తెరవండి

journal.window.facets.curation.suggestions.open.activity = కార్యకలాపం తెరవండి

journal.window.facets.curation.suggestions.open.duplicates = సాధ్యమైన నకలులు తెరవండి

journal.window.facets.curation.suggestions.open.lines = పంక్తులు తెరవండి

journal.window.facets.curation.suggestions.open.person.name = { $person_name } తెరవండి

journal.window.facets.curation.suggestions.person.facet.membership = { $facet_list } లో

journal.window.facets.curation.suggestions.person.notes.count = { $quantity ->
    [one] 1 నోట్
   *[other] { $count } నోట్‌లు
}

journal.window.facets.curation.suggestions.receipt.accepted.alias = { $agent_name } సూచన అంగీకరించబడింది. { $former_name } ఇప్పుడు { $new_name } అని కూడా పిలవబడుతుంది.

journal.window.facets.curation.suggestions.receipt.accepted.ask = { $agent_name } సూచన అంగీకరించబడింది. మీ జర్నల్ దీన్ని మళ్ళీ ఆలోచిస్తుంది.

journal.window.facets.curation.suggestions.receipt.accepted.ask.unconfirmed = { $agent_name } సూచన అంగీకరించి సేవ్ చేయబడింది, కాని దీన్ని మళ్ళీ ఆలోచిస్తుందని మీ జర్నల్ నిర్ధారించలేకపోయింది.

journal.window.facets.curation.suggestions.receipt.accepted.merged.people = { $agent_name } సూచన అంగీకరించబడింది. { $merged_name } మరియు { $kept_name } ఇప్పుడు ఒకే వ్యక్తి, పేరు { $kept_name }.

journal.window.facets.curation.suggestions.receipt.accepted.partial = ఈ కార్యకలాపం కోసం { $agent_name } సూచన అంగీకరించబడింది. మరికొన్ని కార్యకలాపాలు అప్‌డేట్ చేయలేకపోయాయి.

journal.window.facets.curation.suggestions.receipt.accepted.rename = { $agent_name } సూచన అంగీకరించబడింది. { $former_name } ఇప్పుడు { $new_name }.

journal.window.facets.curation.suggestions.receipt.accepted.simple = { $agent_name } సూచన అంగీకరించబడింది.

journal.window.facets.curation.suggestions.receipt.dismissed.agent = వదిలివేయబడింది. { $agent_name } దీన్ని మళ్ళీ సూచిస్తే, మీరు ఇప్పటికే నిరాకరించారని { $agent_name } కు చెప్పబడుతుంది.

journal.window.facets.curation.suggestions.receipt.undone.alias = అన్‌డూ అయింది. { $new_name } ఇక { $former_name } కు మరో పేరు కాదు.

journal.window.facets.curation.suggestions.receipt.undone.rename = అన్‌డూ అయింది. { $new_name } మళ్ళీ { $former_name }.

journal.window.facets.curation.suggestions.retry = మళ్ళీ ప్రయత్నించండి

journal.window.facets.curation.suggestions.show.all = ప్రతి ఏజెంట్‌ను చూపండి

journal.window.facets.curation.suggestions.show.more = మరిన్ని చూపండి

journal.window.facets.curation.suggestions.stale.changed.after.agent = { $agent_name } సూచించిన తర్వాత ఇది మారింది, కాబట్టి అంగీకరించేందుకు ఏమీ లేదు. ఇప్పుడు ఇలా ఉంది:

journal.window.facets.curation.suggestions.stale.chip = ఇప్పటికే మారింది

journal.window.facets.curation.suggestions.stale.pair.changed.after.agent = { $agent_name } దీన్ని సూచించిన తర్వాత { $first_name } లేదా { $second_name } మారింది, కాబట్టి అంగీకరించేందుకు ఏమీ లేదు.

journal.window.facets.curation.suggestions.stale.pair.journal.candidate = { $first_name } మరియు { $second_name } ఒకే వ్యక్తేనా అని మీ జర్నల్ ఇప్పటికే అడుగుతోంది. మీరు సాధ్యమైన నకలుల్లో దీనికి సమాధానం ఇవ్వవచ్చు.

journal.window.facets.curation.suggestions.stale.pair.journal.question = { $first_name } మరియు { $second_name } గురించి మీ జర్నల్‌కు సొంత ప్రశ్న ఉంది, కాబట్టి ఇక్కడ అంగీకరించేందుకు ఏమీ లేదు.

journal.window.facets.curation.suggestions.stale.pair.owner.answered = మీరు ఇప్పటికే { $first_name } మరియు { $second_name } విడివిడిగా ఉంచారు, కాబట్టి అంగీకరించేందుకు ఏమీ లేదు.

journal.window.facets.curation.suggestions.suggested = సూచన

journal.window.facets.curation.suggestions.then = మీరు అంగీకరిస్తే

journal.window.facets.curation.suggestions.then.account.days = మీ జర్నల్ ఈ కార్యకలాపం గురించి మీ వృత్తాంతంతో మళ్ళీ ఆలోచిస్తుంది, మరియు { $affected_days } గురించి.

journal.window.facets.curation.suggestions.then.activities.days = { $quantity ->
    [one] మీ జర్నల్ ఈ కార్యకలాపం గురించి మళ్ళీ ఆలోచిస్తుంది, మరియు { $affected_days } గురించి.
   *[other] మీ జర్నల్ ఈ { $count } కార్యకలాపాల గురించి మళ్ళీ ఆలోచిస్తుంది, మరియు { $affected_days } గురించి.
}

journal.window.facets.curation.suggestions.then.alias.name = మీ జర్నల్ ఈ వ్యక్తిని { $new_name } అని కూడా పిలుస్తుంది. ముందటి వెర్షన్ వెర్షన్ చరిత్రలో ఉంటుంది, అక్కడ మీరు దాన్ని రీస్టోర్ చేయవచ్చు.

journal.window.facets.curation.suggestions.then.merge.people = మీ జర్నల్ { $kept_name } అనే ఒక వ్యక్తిని ఉంచుకుంటుంది, { $merged_name } అని కూడా, మరియు { $merged_name } కు జోడించినదంతా వారిపైకి తరలిస్తుంది. జతను అన్‌డూ చేయలేం.

journal.window.facets.curation.suggestions.then.retain.previous.name = ఇకపై మీ జర్నల్ ఈ వ్యక్తిని { $new_name } అని పిలుస్తుంది. { $former_name } వెర్షన్ చరిత్రలో ఉంటుంది, అక్కడ మీరు దాన్ని రీస్టోర్ చేయవచ్చు.

journal.window.facets.curation.suggestions.undo = అన్‌డూ చేయండి

journal.window.facets.curation.suggestions.undone.plain = అన్‌డూ అయింది.

journal.window.facets.curation.suggestions.updating = అప్‌డేట్ అవుతోంది…

journal.window.facets.curation.suggestions.why = ఎందుకు

journal.window.facets.curation.text.loading.suggestions = సూచనలు లోడ్ అవుతున్నాయి…

journal.window.facets.curation.visible.text.cluster = సమూహం:

journal.window.facets.curation.visible.text.could.not.load.suggestions = సూచనలు లోడ్ చేయలేకపోయింది.

journal.window.facets.curation.visible.text.day = రోజు:

journal.window.facets.curation.visible.text.loading.suggestions = సూచనలు లోడ్ అవుతున్నాయి…

journal.window.facets.curation.visible.text.segment = సెగ్మెంట్:

journal.window.facets.curation.visible.text.source = మూలం:

journal.window.facets.curation.visible.text.stream = ప్రవాహం:

journal.window.facets.curation.window.explanation.a.close.spelling.match.was.chosen = దగ్గరి స్పెల్లింగ్ సరిపోవడం స్వయంచాలకంగా ఎంచుకోబడింది

journal.window.facets.curation.window.explanation.couldn.t.accept.that.so.nothing = దాన్ని అంగీకరించలేకపోయింది, కాబట్టి ఏమీ మారలేదు. కాసేపట్లో మళ్ళీ ప్రయత్నించండి.

journal.window.facets.curation.window.explanation.couldn.t.load.what.your.agents = మీ ఏజెంట్‌లు సూచించినది లోడ్ చేయలేకపోయింది.

journal.window.facets.curation.window.explanation.couldn.t.undo.that.because.you = దాన్ని అన్‌డూ చేయలేకపోయింది, ఎందుకంటే ఆ తర్వాత మీరు దాన్ని మార్చారు. మీ మార్పు ఉంటుంది.

journal.window.facets.curation.window.explanation.earlier.from.your.agents = మీ ఏజెంట్‌ల నుండి ముందటివి

journal.window.facets.curation.window.explanation.merge.them.into.one.person = వారిని ఒకే వ్యక్తిగా జత చేయండి

journal.window.facets.curation.window.explanation.possible.duplicate.speaker.names = సాధ్యమైన నకలు మాట్లాడిన వారి పేర్లు

journal.window.facets.curation.window.explanation.saved.for.some.activities = కొన్ని కార్యకలాపాలకు సేవ్ అయింది

journal.window.facets.curation.window.explanation.that.didn.t.go.through.try = అది జరగలేదు. మళ్ళీ ప్రయత్నించండి.

journal.window.facets.curation.window.explanation.the.names.have.little.in.common = పేర్లకు ఉమ్మడి తక్కువ

journal.window.facets.curation.window.explanation.the.suggested.entity.is.no.longer = సూచించిన ఎంటిటీ ఇక ఇక్కడ లేదు

journal.window.facets.curation.window.explanation.the.suggested.entity.is.not.a = సూచించిన ఎంటిటీ వ్యక్తి కాదు

journal.window.facets.curation.window.explanation.this.can.t.be.accepted.right = దీన్ని ఇప్పుడు అంగీకరించలేం.

journal.window.facets.curation.window.explanation.this.was.a.recording.or.video = ఇది ప్లే అవుతున్న రికార్డింగ్ లేదా వీడియో

journal.window.facets.curation.window.explanation.undone.but.your.journal.couldn.t = అన్‌డూ అయింది, కాని దీన్ని మళ్ళీ ఆలోచిస్తుందని మీ జర్నల్ నిర్ధారించలేకపోయింది.

journal.window.facets.curation.window.explanation.undone.your.journal.will.think.about = అన్‌డూ అయింది. ఆ మార్పు లేకుండా మీ జర్నల్ దీన్ని మళ్ళీ ఆలోచిస్తుంది.

journal.window.facets.curation.window.explanation.what.you.accept.is.kept.as = మీరు అంగీకరించింది మీ సొంత వృత్తాంతంగా ఉంచబడుతుంది.

journal.window.facets.curation.window.explanation.you.accepted.it.then.undid.it = మీరు దీన్ని అంగీకరించి, తర్వాత అన్‌డూ చేశారు

journal.window.facets.curation.window.explanation.you.kept.them.separate = మీరు వారిని విడివిడిగా ఉంచారు

journal.window.facets.curation.window.explanation.your.account.of.it = దాని గురించి మీ వృత్తాంతం

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so = మీ జర్నల్ సమాధానం ఇవ్వలేదు, కాబట్టి ఇది ఇప్పటికే పూర్తి కావచ్చు. మళ్ళీ ప్రయత్నిస్తే రెండుసార్లు జరగదు.

journal.window.facets.curation.window.explanation.your.journal.didn.t.answer.so.variant2 = మీ జర్నల్ సమాధానం ఇవ్వలేదు, కాబట్టి ఇది ఇప్పటికే అంగీకరించబడి ఉండవచ్చు. మళ్ళీ ప్రయత్నిస్తే రెండుసార్లు అంగీకరించదు.

journal.window.facets.curation.window.explanation.your.journal.will.think.this.through = మీరు చూసిన లేదా విన్న దానిగా, వేరే ఎవరూ పాల్గొనకుండా, మీ జర్నల్ దీన్ని మళ్ళీ ఆలోచిస్తుంది.

journal.window.location.day.area.unnamed = దగ్గర పేరున్న పట్టణం లేదు ({ $area })

journal.window.location.day.empty = లొకేషన్ ఉంచబడింది, కాని ఏ కొలతకు పేరు పెట్టలేకపోయింది.

journal.window.location.day.heading = { $date } నాటి లొకేషన్

journal.window.location.day.instant = { $time } కు

journal.window.location.day.moment = { $date } నాడు { $clock } కు

journal.window.location.day.nearby.list = { $leading } మరియు { $last } దగ్గర

journal.window.location.day.place.context = { $specific }; { $broader }

journal.window.location.day.place.inside = { $inside } లో { $place } వద్ద

journal.window.location.day.place.named = { $owner_name } వద్ద (మీరు పేరు పెట్టిన ప్రదేశం); { $location }

journal.window.location.day.readings.area = { $quantity ->
    [one] కొలతలు { $area }, { $when }: { $device } నుండి { $count } కొలత.
   *[other] కొలతలు { $area }, { $when }: { $device } నుండి { $count } కొలతలు.
}

journal.window.location.day.readings.area.approximate = { $quantity ->
    [one] కొలతలు { $area }, { $when }: { $device } నుండి { $count } సుమారు కొలత.
   *[other] కొలతలు { $area }, { $when }: { $device } నుండి { $count } సుమారు కొలతలు.
}

journal.window.location.day.readings.unlocated = { $quantity ->
    [one] కొలతలు, { $when }: { $device } నుండి { $count } కొలత.
   *[other] కొలతలు, { $when }: { $device } నుండి { $count } కొలతలు.
}

journal.window.location.day.readings.unlocated.approximate = { $quantity ->
    [one] కొలతలు, { $when }: { $device } నుండి { $count } సుమారు కొలత.
   *[other] కొలతలు, { $when }: { $device } నుండి { $count } సుమారు కొలతలు.
}

journal.window.location.day.stay = మీ జర్నల్ { $device } నుండి { $count } కొలతల నుండి తేల్చిన ఆగిన చోటు { $location }: మొదటి కొలత { $first }, చివరి { $last }.

journal.window.location.day.visit = { $device } నివేదించిన సందర్శన { $location }: { $arrival }, { $departure }.

journal.window.map.accessible.name.edit.this.place = ఈ ప్రదేశాన్ని సవరించండి

journal.window.map.accessible.name.location.accuracy = లొకేషన్ ఖచ్చితత్వం

journal.window.map.accessible.name.map = మ్యాప్

journal.window.map.accessible.name.map.of.the.records.listed.here = ఇక్కడ జాబితాలో ఉన్న నమోదుల మ్యాప్, పట్టణాలు మరియు సరిహద్దులతో

journal.window.map.accessible.name.more.of.your.location = మీ లొకేషన్‌లో మరిన్ని

journal.window.map.accessible.name.other.weeks.and.this.month = ఇతర వారాలు మరియు ఈ నెల

journal.window.map.accessible.name.the.day.in.order = రోజు, క్రమంలో

journal.window.map.accessible.name.the.stops.across.the.day.shown = రోజంతా ఆగిన చోట్లు, కింద జాబితాలో చూపబడ్డాయి

journal.window.map.accessible.name.visits.and.stays.across.the.day = రోజంతా సందర్శనలు మరియు ఆగిన చోట్లు

journal.window.map.accessible.name.zoom = జూమ్

journal.window.map.accessible.name.zoom.in = జూమ్ దగ్గరకు

journal.window.map.accessible.name.zoom.out = జూమ్ దూరంగా

journal.window.map.accuracy.download.button = { $size } డౌన్‌లోడ్ చేయండి

journal.window.map.accuracy.download.explanation = ఇది{ $slot1 }{ $size }{ $slot2 }{ $slot3 } నుండి, అందరికీ ఒకే డౌన్‌లోడ్, కాబట్టి దీన్ని తెచ్చుకోవడం మీరు ఎక్కడ ఉన్నారో ఏమీ చెప్పదు. పేర్లు మీ కంప్యూటర్‌లో లుకప్ అవుతాయి.

journal.window.map.accuracy.download.progress = లొకేషన్ ఖచ్చితత్వం డౌన్‌లోడ్ అవుతోంది:{ $slot1 }{ $total }{ $slot2 }లో{ $slot3 }{ $received }. డౌన్‌లోడ్ అవుతున్నప్పుడు మీరు మీ జర్నల్ వాడవచ్చు.

journal.window.map.accuracy.explanation.names = మీ ఫోన్ నివేదించే సందర్శనకు, మీ ఫోన్ దోష పరిధిలోని ప్రదేశాల పేరు పెట్టబడుతుంది: "{ $slot1 } దగ్గర".

journal.window.map.accuracy.explanation.outlines = మీ ఫోన్ నివేదించే సందర్శనకు, అది ఉన్న భవనం, పార్క్ లేదా క్యాంపస్ పేరు, మరియు మీ ఫోన్ దోష పరిధిలోని ప్రదేశాల పేరు పెట్టబడుతుంది: "{ $slot1 } వద్ద", "{ $slot2 } దగ్గర".

journal.window.map.accuracy.offered.names = లొకేషన్ ఖచ్చితత్వంతో, మీ ఫోన్ నివేదించే సందర్శనకు మీ ఫోన్ దోష పరిధిలోని ప్రదేశాల పేరు పెట్టబడుతుంది: "{ $slot1 } దగ్గర".

journal.window.map.accuracy.offered.outlines = లొకేషన్ ఖచ్చితత్వంతో, మీ ఫోన్ నివేదించే సందర్శనకు, అది ఉన్న భవనం, పార్క్ లేదా క్యాంపస్ పేరు, మరియు మీ ఫోన్ దోష పరిధిలోని ప్రదేశాల పేరు పెట్టబడుతుంది: "{ $slot1 } వద్ద", "{ $slot2 } దగ్గర".

journal.window.map.accuracy.remove.effect = { $storage } ఖాళీ అవుతుంది; పేర్లు దగ్గరి పట్టణానికి మళ్ళీ వెళ్తాయి.

journal.window.map.accuracy.status.verification.unavailable = ఇన్‌స్టాల్ అయిన ఖచ్చితత్వ ప్యాక్‌లు ధృవీకరించలేకపోయాయి

journal.window.map.accuracy.storage.insufficient = తగినంత ఖాళీ చోటు లేదు: లొకేషన్ ఖచ్చితత్వానికి { $required_storage } ఖాళీ కావాలి, మరియు మిగిలేందుకు కొంత చోటు.

journal.window.map.accuracy.update.available = లొకేషన్ ఖచ్చితత్వం ఆన్‌లో ఉంది. updates.solstone.app నుండి కొత్త డౌన్‌లోడ్ సిద్ధంగా ఉంది: { $size }.

journal.window.map.area.label.at.place = { $name }{ $slot1 }వద్ద

journal.window.map.area.label.inside = { $place }{ $slot1 }లో

journal.window.map.area.label.inside.qualified = { $place }{ $slot2 }{ $qualifier }{ $slot1 }లో

journal.window.map.area.label.near = { $place }{ $slot2 }{ $qualifier }{ $slot1 }దగ్గర

journal.window.map.area.label.near.place = { $name }{ $slot1 }దగ్గర

journal.window.map.area.label.qualified = { $region }{ $slot1 }{ $country }

journal.window.map.area.no.town = దగ్గర పేరున్న పట్టణం లేని ప్రదేశం

journal.window.map.area.size = దాదాపు{ $slot1 }{ $size }{ $slot2 }km

journal.window.map.attribution.base = పట్టణాలు { $geonames_start }GeoNames{ $geonames_end } నుండి ({ $licence_start }CC BY 4.0{ $licence_end }) · సరిహద్దులు Natural Earth నుండి

journal.window.map.attribution.outlines = పట్టణాలు { $geonames_start }GeoNames{ $geonames_end } నుండి ({ $licence_start }CC BY 4.0{ $licence_end }) · సరిహద్దులు Natural Earth నుండి · ప్రదేశాలు మరియు అవుట్‌లైన్‌లు { $overture_start }Overture Maps{ $overture_end } నుండి

journal.window.map.attribution.places = పట్టణాలు { $geonames_start }GeoNames{ $geonames_end } నుండి ({ $licence_start }CC BY 4.0{ $licence_end }) · సరిహద్దులు Natural Earth నుండి · ప్రదేశాలు { $overture_start }Overture Maps{ $overture_end } నుండి

journal.window.map.button.label.clear = క్లియర్

journal.window.map.button.label.download.again = మళ్ళీ డౌన్‌లోడ్ చేయండి

journal.window.map.button.label.fit = సరిపెట్టండి

journal.window.map.button.label.remove.location.accuracy = లొకేషన్ ఖచ్చితత్వం తీసివేయండి

journal.window.map.button.label.save = సేవ్ చేయండి

journal.window.map.button.label.show.on.the.map = మ్యాప్‌లో చూపండి

journal.window.map.button.label.update = అప్‌డేట్ చేయండి

journal.window.map.calendar.day.name = { $state ->
    [stops] { $date }, ఆగిన చోట్లు
    [elsewhere] { $date }, వేరే చోట ఆగిన చోట్లు: { $places }
    [no-stops] { $date }, ఆగిన చోట్లు లేవు
    [no-location] { $date }, లొకేషన్ ఉంచబడలేదు
    [unreadable] { $date }, లొకేషన్ చదవలేకపోయింది
    [kept] { $date }, లొకేషన్ ఉంచబడింది
   *[unknown] { $date }, { $state_label }
}

journal.window.map.calendar.day.name.selected = { $state ->
    [stops] { $date }, ఆగిన చోట్లు, ఎంచుకున్న ప్రాంతంలో ఆగిన చోటు ఉంది
    [elsewhere] { $date }, వేరే చోట ఆగిన చోట్లు: { $places }, ఎంచుకున్న ప్రాంతంలో ఆగిన చోటు ఉంది
    [no-stops] { $date }, ఆగిన చోట్లు లేవు, ఎంచుకున్న ప్రాంతంలో ఆగిన చోటు ఉంది
    [no-location] { $date }, లొకేషన్ ఉంచబడలేదు, ఎంచుకున్న ప్రాంతంలో ఆగిన చోటు ఉంది
    [unreadable] { $date }, లొకేషన్ చదవలేకపోయింది, ఎంచుకున్న ప్రాంతంలో ఆగిన చోటు ఉంది
    [kept] { $date }, లొకేషన్ ఉంచబడింది, ఎంచుకున్న ప్రాంతంలో ఆగిన చోటు ఉంది
   *[unknown] { $date }, { $state_label }, ఎంచుకున్న ప్రాంతంలో ఆగిన చోటు ఉంది
}

journal.window.map.calendar.group.accessible.name = { $month }{ $slot1 }రోజులు. రోజుల మధ్య వెళ్ళడానికి బాణం కీలు వాడండి.

journal.window.map.calendar.location.kept = లొకేషన్ ఉంచబడింది

journal.window.map.calendar.location.none = లొకేషన్ ఉంచబడలేదు

journal.window.map.calendar.stops.elsewhere = వేరే చోట ఆగిన చోట్లు

journal.window.map.calendar.stops.here = ఇక్కడ ఆగిన చోట్లు

journal.window.map.calendar.stops.none = ఆగిన చోట్లు లేవు

journal.window.map.count.area = { $quantity ->
    [one] { $count } ప్రాంతం
   *[other] { $count } ప్రాంతాలు
}

journal.window.map.count.day = { $quantity ->
    [one] { $count } రోజు
   *[other] { $count } రోజులు
}

journal.window.map.count.night = { $quantity ->
    [one] { $count } రాత్రి
   *[other] { $count } రాత్రులు
}

journal.window.map.count.reading = { $quantity ->
    [one] { $count } కొలత
   *[other] { $count } కొలతలు
}

journal.window.map.count.record = { $quantity ->
    [one] { $count } నమోదు
   *[other] { $count } నమోదులు
}

journal.window.map.count.visit = { $quantity ->
    [one] { $count } సందర్శన
   *[other] { $count } సందర్శనలు
}

journal.window.map.date.long = { $weekday },{ $slot1 }{ $day }{ $slot2 }{ $month }

journal.window.map.date.short = { $day }{ $slot1 }{ $month }

journal.window.map.date.short.weekday = { $weekday }{ $slot1 }{ $date }

journal.window.map.date.short.weekday.comma = { $weekday },{ $slot1 }{ $date }

journal.window.map.date.time = { $day }{ $slot1 }{ $clock }

journal.window.map.date.time.range = { $first }{ $slot1 }నుండి{ $slot2 }{ $last } వరకు

journal.window.map.day.location.none = { $date } నుండి మీ జర్నల్‌లో లొకేషన్ లేదు. లొకేషన్ పాజ్ అయి ఉండవచ్చు లేదా ఆఫ్‌లో ఉండవచ్చు, లేదా ఫోన్ మీ జర్నల్‌కు చేరకపోయి ఉండవచ్చు. ఏది అని మీ జర్నల్‌కు తెలియదు.

journal.window.map.device.android.phone = { $slot1 } ఫోన్

journal.window.map.device.android.tablet = { $slot1 } టాబ్లెట్

journal.window.map.device.detail.tier.balanced = solstone యాప్‌లో వివరం స్థాయి: { $level_start }ప్రదేశాలు + రాకపోకలు{ $level_end }.

journal.window.map.device.detail.tier.full = solstone యాప్‌లో వివరం స్థాయి: { $level_start }పూర్తి చిత్రం{ $level_end }.

journal.window.map.device.detail.tier.light = solstone యాప్‌లో వివరం స్థాయి: { $level_start }ప్రదేశాలు మాత్రమే{ $level_end }.

journal.window.map.device.from = { $device }{ $slot1 }నుండి

journal.window.map.device.from.android.phone = మీ android ఫోన్ నుండి

journal.window.map.device.from.android.phone.named = మీ android ఫోన్ ({ $device_name }) నుండి

journal.window.map.device.from.android.tablet = మీ android టాబ్లెట్ నుండి

journal.window.map.device.from.android.tablet.named = మీ android టాబ్లెట్ ({ $device_name }) నుండి

journal.window.map.device.from.ipad = మీ ipad నుండి

journal.window.map.device.from.ipad.named = మీ ipad ({ $device_name }) నుండి

journal.window.map.device.from.iphone = మీ iphone నుండి

journal.window.map.device.from.iphone.named = మీ iphone ({ $device_name }) నుండి

journal.window.map.device.from.phone = మీ ఫోన్ నుండి

journal.window.map.device.from.phone.named = మీ ఫోన్ ({ $device_name }) నుండి

journal.window.map.device.from.tablet = మీ టాబ్లెట్ నుండి

journal.window.map.device.from.tablet.named = మీ టాబ్లెట్ ({ $device_name }) నుండి

journal.window.map.device.from.watch = మీ వాచ్ నుండి

journal.window.map.device.from.watch.named = మీ వాచ్ ({ $device_name }) నుండి

journal.window.map.device.interrupted.android.phone = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ android ఫోన్ చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ android ఫోన్ చెప్పింది.
}

journal.window.map.device.interrupted.android.phone.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ android ఫోన్ చెప్పింది.

journal.window.map.device.interrupted.android.tablet = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ android టాబ్లెట్ చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ android టాబ్లెట్ చెప్పింది.
}

journal.window.map.device.interrupted.android.tablet.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ android టాబ్లెట్ చెప్పింది.

journal.window.map.device.interrupted.ipad = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ ipad చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ ipad చెప్పింది.
}

journal.window.map.device.interrupted.ipad.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ ipad చెప్పింది.

journal.window.map.device.interrupted.iphone = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ iphone చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ iphone చెప్పింది.
}

journal.window.map.device.interrupted.iphone.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ iphone చెప్పింది.

journal.window.map.device.interrupted.phone = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ ఫోన్ చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ ఫోన్ చెప్పింది.
}

journal.window.map.device.interrupted.phone.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ ఫోన్ చెప్పింది.

journal.window.map.device.interrupted.tablet = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ టాబ్లెట్ చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ టాబ్లెట్ చెప్పింది.
}

journal.window.map.device.interrupted.tablet.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ టాబ్లెట్ చెప్పింది.

journal.window.map.device.interrupted.unknown = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ { $device } చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ { $device } చెప్పింది.
}

journal.window.map.device.interrupted.unknown.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ { $device } చెప్పింది.

journal.window.map.device.interrupted.watch = { $quantity ->
    [one] ఈ రోజు లొకేషన్‌కు ఒకసారి అంతరాయం కలిగిందని మీ వాచ్ చెప్పింది.
   *[other] ఈ రోజు లొకేషన్‌కు { $count } సార్లు అంతరాయం కలిగిందని మీ వాచ్ చెప్పింది.
}

journal.window.map.device.interrupted.watch.twice = ఈ రోజు లొకేషన్‌కు రెండుసార్లు అంతరాయం కలిగిందని మీ వాచ్ చెప్పింది.

journal.window.map.device.phone = ఫోన్

journal.window.map.device.possessive = మీ{ $slot1 }{ $device }

journal.window.map.device.possessive.named = మీ{ $slot1 }{ $device }{ $slot2 }({ $name })

journal.window.map.device.precision.off = మీ { $device } లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.precision.off.android.phone = మీ android ఫోన్‌లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.precision.off.android.tablet = మీ android టాబ్లెట్‌లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.precision.off.ipad = మీ ipad లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.precision.off.iphone = మీ iphone లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.precision.off.phone = మీ ఫోన్‌లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.precision.off.tablet = మీ టాబ్లెట్‌లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.precision.off.watch = మీ వాచ్‌లో solstone కోసం ఖచ్చితమైన లొకేషన్ ఆఫ్‌లో ఉంది, కాబట్టి దాని కొలతలు పట్టణం పేరు పెడతాయి, భవనం పేరు కాదు.

journal.window.map.device.stay.android.phone = మీ android ఫోన్ కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.android.phone.named = మీ android ఫోన్ ({ $device_name }) కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.android.tablet = మీ android tablet కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.android.tablet.named = మీ android tablet ({ $device_name }) కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.ipad = మీ ipad కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.ipad.named = మీ ipad ({ $device_name }) కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.iphone = మీ iphone కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.iphone.named = మీ iphone ({ $device_name }) కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.phone = మీ ఫోన్ కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.phone.named = మీ ఫోన్ ({ $device_name }) కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.tablet = మీ టాబ్లెట్ కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.tablet.named = మీ టాబ్లెట్ ({ $device_name }) కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.watch = మీ వాచ్ కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.stay.watch.named = మీ వాచ్ ({ $device_name }) కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.device.tablet = టాబ్లెట్

journal.window.map.device.visit.android.phone = మీ android phone నివేదించిన సందర్శన

journal.window.map.device.visit.android.phone.named = మీ android phone ({ $device_name }) నివేదించిన సందర్శన

journal.window.map.device.visit.android.tablet = మీ android tablet నివేదించిన సందర్శన

journal.window.map.device.visit.android.tablet.named = మీ android tablet ({ $device_name }) నివేదించిన సందర్శన

journal.window.map.device.visit.ipad = మీ ipad నివేదించిన సందర్శన

journal.window.map.device.visit.ipad.named = మీ ipad ({ $device_name }) నివేదించిన సందర్శన

journal.window.map.device.visit.iphone = మీ iphone నివేదించిన సందర్శన

journal.window.map.device.visit.iphone.named = మీ iphone ({ $device_name }) నివేదించిన సందర్శన

journal.window.map.device.visit.phone = మీ ఫోన్ నివేదించిన సందర్శన

journal.window.map.device.visit.phone.named = మీ ఫోన్ ({ $device_name }) నివేదించిన సందర్శన

journal.window.map.device.visit.tablet = మీ టాబ్లెట్ నివేదించిన సందర్శన

journal.window.map.device.visit.tablet.named = మీ టాబ్లెట్ ({ $device_name }) నివేదించిన సందర్శన

journal.window.map.device.visit.watch = మీ వాచ్ నివేదించిన సందర్శన

journal.window.map.device.visit.watch.named = మీ వాచ్ ({ $device_name }) నివేదించిన సందర్శన

journal.window.map.disclosure.label.how.places.are.worked.out = ప్రదేశాలు ఎలా తేల్చబడతాయి

journal.window.map.disclosure.label.how.the.month.is.drawn = నెల ఎలా గీయబడుతుంది

journal.window.map.disclosure.label.how.this.is.drawn = ఇది ఎలా గీయబడుతుంది

journal.window.map.disclosure.label.licences = లైసెన్స్‌లు

journal.window.map.disclosure.label.why.these.words = ఈ మాటలు ఎందుకు

journal.window.map.entry.area.town.reason = { $town } ఈ ప్రాంతానికి బాగా నిలిచే పట్టణం, { $distance_km } km దూరంలో ఉంది.

journal.window.map.entry.lines.unreadable = { $quantity ->
    [one] మీ జర్నల్ చదవలేకపోయిన { $count } లైన్
   *[other] మీ జర్నల్ చదవలేకపోయిన { $count } లైన్‌లు
}

journal.window.map.entry.margin.within = ఆ ప్రాంతం మొత్తం { $area } లోపల ఉంది.

journal.window.map.entry.open.arrival = { $departure } వరకు, రాక తెలియదు

journal.window.map.entry.open.departure = { $arrival } నుండి, బయలుదేరడం నివేదించబడలేదు

journal.window.map.entry.precision.approximate = సుమారు

journal.window.map.entry.readings.within = ఇక్కడి ప్రతి కొలత, దాని దోష పరిధితో సహా, { $area } లోపల ఉంది.

journal.window.map.entry.reported.visit = { $device }{ $slot1 }నివేదించిన{ $slot2 }సందర్శన

journal.window.map.entry.stay.margin.inside.place = ఈ కొలతలు, వాటి దోష పరిధులతో సహా, అన్నీ వాటి కేంద్రం నుండి { $margin_meters } m లోపల పడతాయి, అదంతా { $place } లోపల ఉంది.

journal.window.map.entry.stay.margin.near.place = { $place } { $distance_meters } m దూరంలో ఉంది; ఈ కొలతలు, వాటి దోష పరిధులతో సహా, అన్నీ వాటి కేంద్రం నుండి { $margin_meters } m లోపల పడతాయి.

journal.window.map.entry.stay.margin.near.place.category = { $place } ({ $category }) { $distance_meters } m దూరంలో ఉంది; ఈ కొలతలు, వాటి దోష పరిధులతో సహా, అన్నీ వాటి కేంద్రం నుండి { $margin_meters } m లోపల పడతాయి.

journal.window.map.entry.stay.precise.reason = { $quantity ->
    [one] ఈ ఆగిన చోటుకు { $count } ఖచ్చితమైన కొలత జాబితాలో ఉంది.
   *[other] మీ జర్నల్ ఈ ఆగిన చోటును { $count } ఖచ్చితమైన కొలతల నుండి తేల్చింది. అవి మొదటిది నుండి 100 m లోపల కనీసం ఐదు నిమిషాలు ఉన్నాయి. దీని సమయాలు వాటిలో మొదటిది, చివరిది.
}

journal.window.map.entry.stay.recorded.device = { $device } కొలతల నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.entry.stay.within = ఈ కొలతలు, వాటి దోష పరిధులతో సహా, అన్నీ { $area } లోపల పడతాయి.

journal.window.map.entry.time.arrival = చేరింది{ $slot1 }{ $time }

journal.window.map.entry.time.departure = బయలుదేరింది{ $slot1 }{ $time }

journal.window.map.entry.time.missing.arrival = రాక తెలియదు

journal.window.map.entry.time.missing.departure = బయలుదేరడం నివేదించబడలేదు

journal.window.map.entry.transcripts.segments = { $quantity ->
    [one] ట్రాన్స్క్రిప్ట్‌లలో తెరవండి: { $count } సెగ్మెంట్
   *[other] ట్రాన్స్క్రిప్ట్‌లలో తెరవండి: { $count } సెగ్మెంట్‌లు
}

journal.window.map.entry.visit.margin.inside.place = మీ ఫోన్ మిమ్మల్ని { $margin_meters } m లోపల ఉంచింది, అదంతా { $place } లోపల ఉంది.

journal.window.map.entry.visit.margin.near.place = { $place } { $distance_meters } m దూరంలో ఉంది; మీ ఫోన్ మిమ్మల్ని { $margin_meters } m లోపల ఉంచింది.

journal.window.map.entry.visit.margin.near.place.category = { $place } ({ $category }) { $distance_meters } m దూరంలో ఉంది; మీ ఫోన్ మిమ్మల్ని { $margin_meters } m లోపల ఉంచింది.

journal.window.map.entry.visit.margin.too.wide = మీ ఫోన్ మిమ్మల్ని { $margin_meters } m లోపల ఉంచింది, భవనానికి లేదా దగ్గరి ప్రదేశాలకు పేరు పెట్టడానికి చాలా వెడల్పు.

journal.window.map.entry.visit.within = మీ ఫోన్ స్థానం, దాని దోష పరిధితో సహా, { $area } లోపల ఉంది.

journal.window.map.field.label.call.it = దీన్ని పిలవండి

journal.window.map.field.label.hide.this.place = ఈ ప్రదేశాన్ని దాచండి

journal.window.map.field.label.name = పేరు

journal.window.map.heading.couldn.t.open.this.day.s = ఈ రోజు లొకేషన్‌ను తెరవలేకపోయింది.

journal.window.map.heading.no.location.in.your.journal.yet = మీ జర్నల్‌లో ఇంకా లొకేషన్ లేదు

journal.window.map.heading.no.location.kept.for.this.day = ఈ రోజుకు లొకేషన్ ఉంచబడలేదు

journal.window.map.label.about.km = సుమారు 1 km

journal.window.map.leaflet.zoom.in = దగ్గరకు జూమ్ చేయండి

journal.window.map.leaflet.zoom.out = దూరంగా జూమ్ చేయండి

journal.window.map.link.label.next.month = తర్వాతి నెల

journal.window.map.link.label.open.in.transcripts = ట్రాన్స్క్రిప్ట్‌లలో తెరవండి

journal.window.map.link.label.open.the.day = రోజును తెరవండి

journal.window.map.link.label.place = ప్రదేశం

journal.window.map.link.label.places = ప్రదేశాలు

journal.window.map.link.label.previous.month = మునుపటి నెల

journal.window.map.link.label.the.day = రోజు

journal.window.map.link.label.the.week.after = తర్వాతి వారం

journal.window.map.link.label.the.week.before = మునుపటి వారం

journal.window.map.link.label.this.month = ఈ నెల

journal.window.map.link.label.this.week = ఈ వారం

journal.window.map.list.conjunction = { $leading }{ $slot1 }మరియు{ $slot2 }{ $last }

journal.window.map.month.area.around = { $name }{ $slot1 }చుట్టూ

journal.window.map.month.area.clipped = { $month } లోపలి భాగం మాత్రమే లెక్క.

journal.window.map.month.area.days.in.month = { $quantity ->
    [one] ఈ నెలలో { $count } రోజు
   *[other] ఈ నెలలో { $count } రోజులు
}

journal.window.map.month.area.days.ratio = { $days_in_month } లో { $days_with_stops }

journal.window.map.month.area.days.show.all = { $quantity ->
    [one] { $count } రోజు చూపండి
   *[other] { $count } రోజులు అన్నీ చూపండి
}

journal.window.map.month.area.distance.first = { $direction ->
   *[east] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km తూర్పు
    [north-east] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km ఈశాన్యం
    [north] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km ఉత్తరం
    [north-west] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km వాయవ్యం
    [west] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km పడమర
    [south-west] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km నైరుతి
    [south] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km దక్షిణం
    [south-east] జాబితాలో మొదటి ప్రాంతం నుండి { $distance } km ఆగ్నేయం
}

journal.window.map.month.area.distance.month = { $direction ->
   *[east] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km తూర్పు
    [north-east] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km ఈశాన్యం
    [north] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km ఉత్తరం
    [north-west] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km వాయవ్యం
    [west] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km పడమర
    [south-west] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km నైరుతి
    [south] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km దక్షిణం
    [south-east] ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km ఆగ్నేయం
}

journal.window.map.month.area.distance.trip = { $direction ->
   *[east] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km తూర్పు
    [north-east] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km ఈశాన్యం
    [north] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km ఉత్తరం
    [north-west] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km వాయవ్యం
    [west] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km పడమర
    [south-west] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km నైరుతి
    [south] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km దక్షిణం
    [south-east] ఈ ప్రయాణంలో ఎక్కువ రోజులున్న ప్రాంతం నుండి { $distance } km ఆగ్నేయం
}

journal.window.map.month.area.live.days = { $quantity ->
    [one] { $name }, ఈ నెలలో { $count } రోజు
   *[other] { $name }, ఈ నెలలో { $count } రోజులు
}

journal.window.map.month.area.open.stops = { $quantity ->
    [one] 1 సందర్శనకు రాక లేదా బయలుదేరడం లేదు, కాబట్టి దాని సమయం గంటల్లో లేదా రాత్రుల్లో లేదు. దాని రోజు ఇంకా లెక్క.
   *[other] { $count } సందర్శనలకు రాక లేదా బయలుదేరడం లేదు, కాబట్టి వాటి సమయం గంటల్లో లేదా రాత్రుల్లో లేదు. వాటి రోజులు ఇంకా లెక్క.
}

journal.window.map.month.area.size = { $size } వెడల్పు ప్రాంతం

journal.window.map.month.areas.list.region = { $quantity ->
    [one] { $count } ప్రాంతం { $area }, ఎక్కువ రోజులు ముందు.
   *[other] { $count } ప్రాంతాలు { $area }, ఎక్కువ రోజులు ముందు.
}

journal.window.map.month.areas.list.region.home = { $quantity ->
    [one] { $area } చుట్టూ { $count } ప్రాంతం, ఎక్కువ రోజులు ముందు.
   *[other] { $area } చుట్టూ { $count } ప్రాంతాలు, ఎక్కువ రోజులు ముందు.
}

journal.window.map.month.areas.list.region.shared = { $quantity ->
    [one] { $count } ప్రాంతం { $area }, అన్నీ { $region } లో, ఎక్కువ రోజులు ముందు.
   *[other] { $count } ప్రాంతాలు { $area }, అన్నీ { $region } లో, ఎక్కువ రోజులు ముందు.
}

journal.window.map.month.areas.list.region.shared.home = { $quantity ->
    [one] { $area } చుట్టూ { $count } ప్రాంతం, అన్నీ { $region } లో, ఎక్కువ రోజులు ముందు.
   *[other] { $area } చుట్టూ { $count } ప్రాంతాలు, అన్నీ { $region } లో, ఎక్కువ రోజులు ముందు.
}

journal.window.map.month.areas.list.trip = { $quantity ->
    [one] { $date } ప్రయాణంలో { $count } ప్రాంతం, ఎక్కువ రోజులు ముందు.
   *[other] { $date } ప్రయాణంలో { $count } ప్రాంతాలు, ఎక్కువ రోజులు ముందు.
}

journal.window.map.month.areas.list.trip.shared = { $quantity ->
    [one] { $date } ప్రయాణంలో { $count } ప్రాంతం, అన్నీ { $region } లో, ఎక్కువ రోజులు ముందు.
   *[other] { $date } ప్రయాణంలో { $count } ప్రాంతాలు, అన్నీ { $region } లో, ఎక్కువ రోజులు ముందు.
}

journal.window.map.month.areas.live.zoom = { $quantity ->
    [one] { $count } ప్రాంతం మీద దగ్గరకు జూమ్ అయింది
   *[other] { $count } ప్రాంతాల మీద దగ్గరకు జూమ్ అయింది
}

journal.window.map.month.areas.outlined = { $quantity ->
    [one] { $count } ప్రాంతం అవుట్‌లైన్ అయింది
   *[other] { $count } ప్రాంతాలు అవుట్‌లైన్ అయ్యాయి
}

journal.window.map.month.areas.outside.show.all = { $quantity ->
    [one] మ్యాప్ బయట { $count } ప్రాంతం · అన్నీ చూపండి
   *[other] మ్యాప్ బయట { $count } ప్రాంతాలు · అన్నీ చూపండి
}

journal.window.map.month.areas.show.all = { $quantity ->
    [one] { $count } ప్రాంతం చూపండి
   *[other] { $count } ప్రాంతాలు అన్నీ చూపండి
}

journal.window.map.month.choice.trip = { $date }{ $slot1 }ప్రయాణంలో{ $slot2 }

journal.window.map.month.day.location.none = మీ జర్నల్‌లో{ $slot1 }{ $date } నుండి లొకేషన్ లేదు. లొకేషన్ పాజ్ అయి ఉండవచ్చు, లేదా ఆఫ్ అయి ఉండవచ్చు, లేదా ఫోన్ మీ జర్నల్‌కు చేరకపోయి ఉండవచ్చు. మీ జర్నల్‌కు ఏది అనేది తెలియదు.

journal.window.map.month.day.stay.readings = { $quantity ->
    [one] ఈ ఆగిన చోటుకు మీ ఫోన్ నుండి { $count } కొలత జాబితాలో ఉంది
   *[other] మీ ఫోన్ కొలతల్లో { $count } నుండి మీ జర్నల్ తేల్చిన ఆగిన చోటు
}

journal.window.map.month.days.show.fewer = తక్కువ రోజులు చూపండి

journal.window.map.month.explanation.midnight = { $month }{ $slot1 }లోపలి{ $slot2 }సమయం మాత్రమే లెక్క. { $previous }{ $slot3 }{ $day }{ $slot4 }న మొదలైన సందర్శన{ $slot5 }అర్ధరాత్రి నుండి లెక్క.

journal.window.map.month.heading = { $month }{ $slot1 }{ $year }

journal.window.map.month.live.areas.all = ప్రతి ప్రాంతం చూపిస్తోంది

journal.window.map.month.live.choice.none = ఏమీ ఎంచుకోలేదు

journal.window.map.month.live.fit = మ్యాప్ అమర్చబడింది

journal.window.map.month.live.showing.choice = { $name }{ $slot1 }చూపిస్తోంది

journal.window.map.month.live.showing.home = { $area } చుట్టూ చూపిస్తోంది

journal.window.map.month.live.zoom.amount = జూమ్{ $slot1 }{ $zoom }

journal.window.map.month.location.gaps = { $quantity ->
    [one] { $count } రోజున లొకేషన్ ఉంచబడలేదు.
   *[other] { $count } రోజుల్లో లొకేషన్ ఉంచబడలేదు.
}

journal.window.map.month.map.area.summary = { $shaded_quantity ->
    [one] { $area } మ్యాప్: { $shaded_areas } మసక ప్రాంతం, { $size } వెడల్పు. అదే ప్రాంతం ఈ నెల ప్రాంతాల కింద జాబితాలో ఉంది.
   *[other] { $area } మ్యాప్: { $shaded_areas } మసక ప్రాంతాలు, ఒక్కొక్కటి { $size } వెడల్పు. అవే ప్రాంతాలు ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్నాయి.
}

journal.window.map.month.map.area.summary.home = { $shaded_quantity ->
    [one] { $area } చుట్టూ ఉన్న ప్రాంతం మ్యాప్: { $shaded_areas } మసక ప్రాంతం, { $size } వెడల్పు. అదే ప్రాంతం ఈ నెల ప్రాంతాల కింద జాబితాలో ఉంది.
   *[other] { $area } చుట్టూ ఉన్న ప్రాంతం మ్యాప్: { $shaded_areas } మసక ప్రాంతాలు, ఒక్కొక్కటి { $size } వెడల్పు. అవే ప్రాంతాలు ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్నాయి.
}

journal.window.map.month.map.group.summary = { $shaded_quantity ->
    [one] { $area_quantity ->
        [one] { $area } మ్యాప్: { $shaded_areas } మసక ప్రాంతం, { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాన్ని ఉంచుతోంది.
       *[other] { $area } మ్యాప్: { $shaded_areas } మసక ప్రాంతం, { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాలను ఉంచుతోంది.
    }
   *[other] { $area_quantity ->
        [one] { $area } మ్యాప్: { $shaded_areas } మసక ప్రాంతాలు, ఒక్కొక్కటి { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాన్ని ఉంచుతున్నాయి.
       *[other] { $area } మ్యాప్: { $shaded_areas } మసక ప్రాంతాలు, ఒక్కొక్కటి { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాలను ఉంచుతున్నాయి.
    }
}

journal.window.map.month.map.group.summary.home = { $shaded_quantity ->
    [one] { $area_quantity ->
        [one] { $area } చుట్టూ ఉన్న ప్రాంతం మ్యాప్: { $shaded_areas } మసక ప్రాంతం, { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాన్ని ఉంచుతోంది.
       *[other] { $area } చుట్టూ ఉన్న ప్రాంతం మ్యాప్: { $shaded_areas } మసక ప్రాంతం, { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాలను ఉంచుతోంది.
    }
   *[other] { $area_quantity ->
        [one] { $area } చుట్టూ ఉన్న ప్రాంతం మ్యాప్: { $shaded_areas } మసక ప్రాంతాలు, ఒక్కొక్కటి { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాన్ని ఉంచుతున్నాయి.
       *[other] { $area } చుట్టూ ఉన్న ప్రాంతం మ్యాప్: { $shaded_areas } మసక ప్రాంతాలు, ఒక్కొక్కటి { $size } వెడల్పు, ఈ నెల ప్రాంతాల కింద జాబితాలో ఉన్న { $areas } ప్రాంతాలను ఉంచుతున్నాయి.
    }
}

journal.window.map.month.map.nothing.drawn = ఇక్కడ ఏమీ గీయలేదు

journal.window.map.month.readings.undrawn = { $quantity ->
    [one] గీయలేదు: { $count } కొలత.
   *[other] గీయలేదు: { $count } కొలతలు.
}

journal.window.map.month.stops.calendar = { $stops_quantity ->
    [one] { $days_in_month } లో { $days_with_stops } రోజుకు ఆగిన చోట్లు ఉన్నాయి
   *[other] { $days_in_month } లో { $days_with_stops } రోజులకు ఆగిన చోట్లు ఉన్నాయి
}

journal.window.map.month.stops.glance = { $stops_quantity ->
    [one] { $days_in_month } లో { $days_with_stops } రోజుకు ఆగిన చోట్లు ఉన్నాయి.
   *[other] { $days_in_month } లో { $days_with_stops } రోజులకు ఆగిన చోట్లు ఉన్నాయి.
}

journal.window.map.month.stops.glance.home.days = { $stops_quantity ->
    [one] { $days_in_month } లో { $days_with_stops } రోజుకు ఆగిన చోట్లు ఉన్నాయి: { $home_days } { $home_area } చుట్టూ.
   *[other] { $days_in_month } లో { $days_with_stops } రోజులకు ఆగిన చోట్లు ఉన్నాయి: { $home_days } { $home_area } చుట్టూ.
}

journal.window.map.month.stops.glance.trips.days = { $trip_quantity ->
    [one] { $days_in_month } లో { $days_with_stops } రోజులకు ఆగిన చోట్లు { $stops_quantity ->
        [one] ఉంది
       *[other] ఉన్నాయి
    }: { $home_days } { $home_area } చుట్టూ, మరియు 50 km కంటే దూరంగా { $trips } ప్రయాణం.
   *[other] { $days_in_month } లో { $days_with_stops } రోజులకు ఆగిన చోట్లు { $stops_quantity ->
        [one] ఉంది
       *[other] ఉన్నాయి
    }: { $home_days } { $home_area } చుట్టూ, మరియు 50 km కంటే దూరంగా { $trips } ప్రయాణాలు.
}

journal.window.map.month.stops.undrawn.hidden = { $quantity ->
    [one] మీరు దాచిన ప్రదేశం
   *[other] మీరు దాచిన { $count } ప్రదేశాలు
}

journal.window.map.month.stops.undrawn.wide = { $quantity ->
    [one] { $count } ఆగిన చోటు, దోష పరిధి 500 m కంటే ఎక్కువ లేదా నివేదించబడలేదు
   *[other] { $count } ఆగిన చోట్లు, దోష పరిధి 500 m కంటే ఎక్కువ లేదా నివేదించబడలేదు
}

journal.window.map.month.strip.faded.bar = మసక బార్‌కు రాక లేదా బయలుదేరడం లేదు.

journal.window.map.month.strip.visits.and.stays = నిండిన బార్‌లు మీ ఫోన్ నివేదించిన సందర్శనలు; అవుట్‌లైన్ బార్‌లు మీ జర్నల్ తేల్చిన ఆగిన చోట్లు.

journal.window.map.month.time.all.day = రోజంతా

journal.window.map.month.time.from = { $time }{ $slot1 }నుండి

journal.window.map.month.time.until = { $time }{ $slot1 }వరకు

journal.window.map.month.zoom.notice = దగ్గరకు జూమ్ అయింది.

journal.window.map.months.label.april = ఏప్రిల్

journal.window.map.months.label.august = ఆగస్టు

journal.window.map.months.label.december = డిసెంబర్

journal.window.map.months.label.february = ఫిబ్రవరి

journal.window.map.months.label.january = జనవరి

journal.window.map.months.label.july = జూలై

journal.window.map.months.label.june = జూన్

journal.window.map.months.label.march = మార్చి

journal.window.map.months.label.may = మే

journal.window.map.months.label.november = నవంబర్

journal.window.map.months.label.october = అక్టోబర్

journal.window.map.months.label.september = సెప్టెంబర్

journal.window.map.navigation.go.day = { $date } కు వెళ్ళండి

journal.window.map.navigation.go.month = { $month } { $year } కు వెళ్ళండి

journal.window.map.option.label.home = ఇల్లు

journal.window.map.option.label.nothing = ఏమీ

journal.window.map.option.label.work = పని

journal.window.map.place.named.link = మీరు "{ $name }" అని పేరు పెట్టిన ప్రదేశం

journal.window.map.place.named.reason = మీరు ఈ ప్రదేశానికి "{ $name }" అని పేరు పెట్టారు.

journal.window.map.place.recorded.dates = { $quantity ->
    [one] ఇక్కడ { $count } తేదీన సందర్శనలు లేదా ఆగిన చోట్లు.
   *[other] ఇక్కడ { $count } తేదీల్లో సందర్శనలు లేదా ఆగిన చోట్లు.
}

journal.window.map.place.recorded.dates.range = { $quantity ->
    [one] ఇక్కడ { $count } తేదీన సందర్శనలు లేదా ఆగిన చోట్లు, { $first } నుండి { $last } వరకు.
   *[other] ఇక్కడ { $count } తేదీల్లో సందర్శనలు లేదా ఆగిన చోట్లు, { $first } నుండి { $last } వరకు.
}

journal.window.map.place.recorded.dates.single = { $quantity ->
    [one] ఇక్కడ { $count } తేదీన సందర్శనలు లేదా ఆగిన చోట్లు, { $date }.
   *[other] ఇక్కడ { $count } తేదీల్లో సందర్శనలు లేదా ఆగిన చోట్లు, { $date }.
}

journal.window.map.place.recorded.nights = { $quantity ->
    [one] { $count } రాత్రి, ఇక్కడి సందర్శన లేదా ఆగిన చోటు 22:00 నుండి 06:00 మధ్య కనీసం రెండు గంటలు వర్తిస్తుంది.
   *[other] { $count } రాత్రులు, ఇక్కడి సందర్శన లేదా ఆగిన చోటు 22:00 నుండి 06:00 మధ్య కనీసం రెండు గంటలు వర్తిస్తుంది.
}

journal.window.map.place.visibility.hidden = · దాచబడింది

journal.window.map.response.accuracy.busy = లొకేషన్ ఖచ్చితత్వం ఇప్పటికే డౌన్‌లోడ్ అవుతోంది.

journal.window.map.response.accuracy.busy.detail = డౌన్‌లోడ్ నడుస్తోంది

journal.window.map.response.accuracy.download.failed = లొకేషన్ ఖచ్చితత్వం డౌన్‌లోడ్ చేయలేకపోయింది.

journal.window.map.response.accuracy.not.offered = మీ జర్నల్ ఈ వెర్షన్‌లో లొకేషన్ ఖచ్చితత్వం డౌన్‌లోడ్ లేదు.

journal.window.map.response.accuracy.not.offered.detail = ఏమీ ఇవ్వబడలేదు

journal.window.map.response.accuracy.remove.failed = లొకేషన్ ఖచ్చితత్వం తీసివేయలేకపోయింది.

journal.window.map.response.area.unavailable = మీ జర్నల్‌తో వచ్చే ప్రదేశాల పేర్లు చదవలేకపోయాయి.

journal.window.map.response.day.invalid = ఆ రోజును వాడలేకపోయింది.

journal.window.map.response.day.invalid.detail = రోజు దొరకలేదు

journal.window.map.response.edit.alias = { $slot1 } { $slot2 }, { $slot3 } లేదా { $slot4 } అయి ఉండాలి

journal.window.map.response.edit.body = Body ఒక { $slot1 } ఆబ్జెక్ట్ అయి ఉండాలి

journal.window.map.response.edit.failed = ఆ మార్పు సేవ్ చేయలేకపోయింది.

journal.window.map.response.edit.hidden = { $slot1 } { $slot2 } లేదా { $slot3 } అయి ఉండాలి

journal.window.map.response.edit.invalid = ఆ మార్పు వాడలేకపోయింది.

journal.window.map.response.edit.name = { $slot1 } స్ట్రింగ్ లేదా { $slot2 } అయి ఉండాలి

journal.window.map.response.month.invalid = ఆ నెలను వాడలేకపోయింది.

journal.window.map.response.month.invalid.detail = నెల { $slot1 } అయి ఉండాలి

journal.window.map.response.place.unknown = ఆ ప్రదేశం మీ జర్నల్‌లో లేదు.

journal.window.map.response.place.unknown.detail = ప్రదేశం దొరకలేదు

journal.window.map.section.heading.areas.this.month = ఈ నెల ప్రాంతాలు

journal.window.map.section.heading.day.by.day = రోజు రోజుకు

journal.window.map.section.heading.deleting.any.of.this = వీటిలో దేన్నైనా తొలగించడం

journal.window.map.section.heading.edit.this.place = ఈ ప్రదేశాన్ని సవరించండి

journal.window.map.section.heading.location.accuracy = లొకేషన్ ఖచ్చితత్వం

journal.window.map.section.heading.the.day.in.order = రోజు, క్రమంలో

journal.window.map.section.heading.the.map = మ్యాప్

journal.window.map.section.heading.the.month.at.a.glance = ఒక్క చూపులో నెల

journal.window.map.section.heading.visits.and.stays.here = ఇక్కడి సందర్శనలు, ఆగిన చోట్లు

journal.window.map.status.text.couldn.t.open.this.month.s = ఈ నెల లొకేషన్‌ను తెరవలేకపోయింది.

journal.window.map.text.loading = లోడ్ అవుతోంది…

journal.window.map.text.loading.this.day.s.location = ఈ రోజు లొకేషన్ లోడ్ అవుతోంది…

journal.window.map.text.loading.your.journal.s.location = మీ జర్నల్ లొకేషన్ లోడ్ అవుతోంది…

journal.window.map.visible.text.a.hidden.place.is.off.the = దాచబడిన ప్రదేశం మ్యాప్ బయట ఉంటుంది, మరియు మీరు ప్రతి రోజు ఎక్కడ ఉన్నారో మీ జర్నల్ రాసేది దాన్ని మినహాయిస్తుంది: శోధనలో, కనెక్ట్ అయి ఉన్న ఏజెంట్‌లకు, మరియు కార్యకలాపాల్లో. కొత్త రోజువారీ బ్రీఫింగ్‌లు కూడా దాన్ని మినహాయిస్తాయి.

journal.window.map.visible.text.a.night.counts.in.an.area = ఒక ప్రాంతంలో రాత్రి లెక్క, అక్కడి సందర్శన 22:00 నుండి 06:00 మధ్య కనీసం రెండు గంటలు వర్తించినప్పుడు.

journal.window.map.visible.text.a.place.is.a.spot.your = ప్రదేశం అంటే ఒక చోటు. మీ ఫోన్ రెండు వేరు తేదీల్లో అక్కడ సందర్శన నివేదించింది, లేదా అక్కడ ఉంది, ఒకే చోటు కావడానికి అంత దగ్గరగా. సుమారు లొకేషన్ వల్ల ఎప్పుడూ ప్రదేశం రాదు.

journal.window.map.visible.text.a.place.lasts.as.long.as = ప్రదేశం, అది వచ్చిన సెగ్మెంట్ ఉన్నంత వరకు ఉంటుంది. ఆ సెగ్మెంట్‌లను ట్రాన్స్క్రిప్ట్‌లలో తొలగిస్తే ప్రదేశం, దాని పేరు కూడా, పోతాయి.

journal.window.map.visible.text.a.place.starts.from.visits.or = ప్రదేశం రెండు లేదా అంతకంటే ఎక్కువ తేదీల సందర్శనలు లేదా ఆగిన చోట్ల నుండి మొదలవుతుంది. అవన్నీ ఒకదానికొకటి 50 m లోపల, మరే ప్రదేశం నుండీ 100 m కంటే ఎక్కువ. తర్వాత దాని 50 m లోపల వచ్చేవి దానిలో కలుస్తాయి. దోష పరిధి 50 m కంటే ఎక్కువ ఉన్న ఆగిన చోటు లేదా సందర్శన ఎప్పుడూ ఒకదాన్ని చేయదు, కలవదు.

journal.window.map.visible.text.a.place.you.hid.isn.t = మీరు దాచిన ప్రదేశం గీయబడదు. దాని సందర్శనలు ప్రతి రోజు జాబితాలో ఇంకా చూపబడతాయి.

journal.window.map.visible.text.a.stay.is.worked.out.from = ఆగిన చోటు ఖచ్చితమైన కొలతల నుండి తేల్చబడుతుంది. అవి ఒకదాని నుండి మరొకదానికి ఐదు నిమిషాల కంటే ఎక్కువ తేడా లేదు, మొదటిది నుండి 100 m లోపల కనీసం ఐదు నిమిషాలు ఉన్నాయి. సుమారు కొలతల నుండి ఎప్పుడూ ఆగిన చోటు రాదు. దీని సమయాలు మొదటి కొలత, చివరి కొలత.

journal.window.map.visible.text.a.stop.at.one.of.your = మీ ప్రదేశాల్లో ఒకదాని వద్ద ఉన్న ఆగిన చోటు, ఆ ప్రదేశం ఉన్న చోటే గీయబడుతుంది, కాబట్టి ఒక ప్రదేశం రెండు ప్రాంతాల మధ్య చీలదు. దోష పరిధి నివేదించబడని, లేదా 500 m కంటే ఎక్కువ ఉన్న, ఆగిన చోటు గీయబడదు. ఆగిన చోట్ల మధ్య సమయం కూడా గీయబడదు.

journal.window.map.visible.text.also.in.this.area = ఈ ప్రాంతంలో ఇంకా

journal.window.map.visible.text.checking.location.accuracy.before.using.it = దీన్ని వాడే ముందు లొకేషన్ ఖచ్చితత్వం సరిచూస్తోంది. మీ జర్నల్‌ను మీరు వాడుతూనే ఉండవచ్చు.

journal.window.map.visible.text.days.here = ఇక్కడి రోజులు

journal.window.map.visible.text.days.with.a.stop = ఆగిన చోటు ఉన్న రోజులు

journal.window.map.visible.text.each.area.is.about.km.across = ప్రతి ప్రాంతం సుమారు 1 km వెడల్పు, లోపలి ఆగిన చోట్లను ఉంచుతుంది: మీ ఫోన్ నివేదించిన సందర్శనలు, మీ జర్నల్ తేల్చిన ఆగిన చోట్లు.

journal.window.map.visible.text.every.record.above.opens.in.transcripts = పైన ఉన్న ప్రతి నమోదు ట్రాన్స్క్రిప్ట్‌లలో తెరుచుకుంటుంది. అక్కడ దాని సెగ్మెంట్‌లను మీరు మిగతావాటిలాగే తొలగిస్తారు. తొలగించిన సెగ్మెంట్ ఈ చూపు నుండి, శోధన నుండి వెళ్ళిపోతుంది.

journal.window.map.visible.text.filled.bars.are.visits.your.phone = నిండిన బార్‌లు మీ ఫోన్ నివేదించిన సందర్శనలు; అవుట్‌లైన్ బార్‌లు మీ జర్నల్ తేల్చిన ఆగిన చోట్లు. ప్రతి కొలత ప్రతి రోజు పేజీలో ఉంటుంది.

journal.window.map.visible.text.for.readings.the.times.are.the = కొలతలకు, సమయాలు వరుసలో మొదటి కొలత, చివరి కొలత. అవి మీరు ఎక్కడైనా ఎంతసేపు ఉన్నారో చెప్పవు.

journal.window.map.visible.text.hidden = దాచబడింది

journal.window.map.visible.text.hours.are.shown.for.one.area = గంటలు ఒకేసారి ఒక ప్రాంతానికి చూపబడతాయి, రెండు చివరలు తెలిసిన సమయానికి మాత్రమే. ప్రాంతాల మధ్య అవి కలపబడవు, ఎందుకంటే రెండు నివేదికలు ఒకే గంటను రెండు ప్రాంతాల్లో పెట్టవచ్చు.

journal.window.map.visible.text.hours.with.both.ends.known = రెండు చివరలు తెలిసిన గంటలు

journal.window.map.visible.text.location.accuracy.is.on = లొకేషన్ ఖచ్చితత్వం ఆన్.

journal.window.map.visible.text.location.accuracy.is.unavailable = లొకేషన్ ఖచ్చితత్వం అందుబాటులో లేదు.

journal.window.map.visible.text.location.may.have.been.paused.or = లొకేషన్ పాజ్ అయి ఉండవచ్చు, లేదా ఆఫ్ అయి ఉండవచ్చు, లేదా ఫోన్ మీ జర్నల్‌కు చేరకపోయి ఉండవచ్చు. మీ జర్నల్‌కు ఏది అనేది తెలియదు.

journal.window.map.visible.text.location.was.kept.this.day.but = ఈ రోజు లొకేషన్ ఉంచబడింది, కానీ ఆగిన చోట్లు లేవు.

journal.window.map.visible.text.named.features.only.streets.not.shown = పేరు ఉన్న ఫీచర్‌లు మాత్రమే; వీధులు చూపబడవు.

journal.window.map.visible.text.nights = రాత్రులు

journal.window.map.visible.text.no.location.kept.for.this.day = ఈ రోజుకు లొకేషన్ ఉంచబడలేదు.

journal.window.map.visible.text.no.location.kept.for.this.month = ఈ నెలకు లొకేషన్ ఉంచబడలేదు.

journal.window.map.visible.text.no.places.yet = ఇంకా ప్రదేశాలు లేవు.

journal.window.map.visible.text.no.stops.to.draw.this.month = ఈ నెల గీయడానికి ఆగిన చోట్లు లేవు.

journal.window.map.visible.text.not.drawn.here = ఇక్కడ గీయలేదు:

journal.window.map.visible.text.nothing.drawn.here = · ఇక్కడ ఏమీ గీయలేదు

journal.window.map.visible.text.on.the.map.a.run.of = మ్యాప్‌పై ఖచ్చితమైన కొలతల వరుస, మధ్య కొలత వద్ద ఒక చుక్కగా కనిపిస్తుంది, కాబట్టి రోజు మార్గంగా కనబడదు. జాబితా ప్రతి కొలతను లెక్కిస్తుంది.

journal.window.map.visible.text.saved = సేవ్ అయింది.

journal.window.map.visible.text.shades.run.from.day.lightest.through = మసకలు 1 రోజు నుండి, అతి లేతది, 2 నుండి 3, 4 నుండి 7, 8 నుండి 14 గుండా, 15 లేదా అంతకంటే ఎక్కువ రోజుల వరకు, అతి ముదురు, ఉంటాయి.

journal.window.map.visible.text.show.fewer = తక్కువ చూపండి

journal.window.map.visible.text.stops = ఆగిన చోట్లు

journal.window.map.visible.text.stops.in.order = ఆగిన చోట్లు, క్రమంలో

journal.window.map.visible.text.that.change.couldn.t.be.saved = ఆ మార్పు సేవ్ చేయలేకపోయింది.

journal.window.map.visible.text.the.shade.counts.days.not.hours = మసక రోజులను లెక్కిస్తుంది, గంటలను కాదు. ఒక రోజు ఒక ప్రాంతంలో ఒకసారి లెక్క, నివేదికలు ఎన్ని ఉన్నా. రాక లేదా బయలుదేరడం తెలియనప్పుడు కూడా సందర్శన తన రోజుకు లెక్క.

journal.window.map.visible.text.the.stops.your.journal.holds.for = ఈ నెలకు మీ జర్నల్ ఉంచుకున్న ఆగిన చోట్లు. ప్రతి ప్రాంతం, దానిలో ఆగిన చోటు ఉన్న రోజుల సంఖ్యతో మసక అవుతుంది.

journal.window.map.visible.text.there.s.no.street.map.a = వీధి మ్యాప్ లేదు. వీధి మ్యాప్ ఒక మ్యాప్ సేవ నుండి ముక్క ముక్కగా లోడ్ అవుతుంది, సరైన ముక్క అడిగితే ఆ సేవకు మీరు ఎక్కడ ఉన్నారో తెలుస్తుంది.

journal.window.map.visible.text.there.s.no.street.map.the = వీధి మ్యాప్ లేదు. పేర్లు మీ జర్నల్‌తో వచ్చినవి, లేదా మీరు డౌన్‌లోడ్ చేసినవి. ఏ మ్యాప్ సేవనూ ఏమీ అడగదు.

journal.window.map.visible.text.to.remove.a.visit.or.stay = సందర్శనను లేదా ఆగిన చోటును తీసివేయడానికి, దాని రోజును తెరిచి, ట్రాన్స్క్రిప్ట్‌లలో దాని సెగ్మెంట్‌లను తొలగించండి. ప్రదేశం, అది వచ్చిన సెగ్మెంట్ ఉన్నంత వరకు ఉంటుంది.

journal.window.map.visible.text.what.you.call.a.place.a = మీరు ప్రదేశాన్ని ఏమని పిలుస్తారో, పేరు గానీ ఇల్లు గానీ పని గానీ, శోధనలో వాడబడుతుంది, మీ జర్నల్ సొంత ఆలోచనలో కూడా. మీ జర్నల్ అంతటా ట్రాన్స్క్రిప్ట్‌లు చూడగల ఏజెంట్ దాన్ని చదవగలదు.

journal.window.map.visible.text.where = ఎక్కడ

journal.window.map.visible.text.where.starts.around.the.area.with = "ఎక్కడ" ఎక్కువ రోజులున్న ప్రాంతం చుట్టూ మొదలవుతుంది: దాని 50 km లోపల ప్రతి ఆగిన చోటు. అంతకంటే దూరంగా ఆగిన చోటు ఉన్న రోజుల వరుస ఒక ప్రయాణం, దాని తేదీల పేరుతో.

journal.window.map.visible.text.with.location.accuracy.a.visit.can = లొకేషన్ ఖచ్చితత్వంతో, సందర్శన మీ ఫోన్ దోష పరిధి లోపల ఉన్న ప్రదేశాలను "దగ్గర" అనవచ్చు. ప్రతి నమోదు అది అలా ఎందుకు ఉందో చెబుతుంది.

journal.window.map.visible.text.with.location.accuracy.a.visit.can.variant2 = లొకేషన్ ఖచ్చితత్వంతో, మీ ఫోన్ దోష పరిధి అంతా ఒక ప్రదేశం అవుట్‌లైన్ లోపల ఉన్నప్పుడు సందర్శన ఆ ప్రదేశాన్ని "వద్ద" అనవచ్చు, ఆ పరిధి లోపల ఉన్న ప్రదేశాలను "దగ్గర" అనవచ్చు. "లో" అంటే పరిధి అంతా పెద్ద అవుట్‌లైన్ లోపల ఉండటం, నివాస ప్రాంతం లేదా పట్టణం. ప్రతి నమోదు అది అలా ఎందుకు ఉందో చెబుతుంది.

journal.window.map.visible.text.without.it.a.visit.is.named = అది లేకుండా, సందర్శనకు దగ్గరి పట్టణం పేరు పెట్టబడుతుంది.

journal.window.map.visible.text.without.location.accuracy.each.reading.is = లొకేషన్ ఖచ్చితత్వం లేకుండా, ప్రతి కొలతకు దాని ప్రాంతానికి బాగా నిలిచే పట్టణం పేరు. అది మీరు ఉన్న పట్టణం కాకపోవచ్చు. అందుకే అది "దగ్గర" అంటుంది.

journal.window.map.visible.text.you.can.name.a.place.call = మీరు ఒక ప్రదేశానికి పేరు పెట్టవచ్చు, దీన్ని ఇల్లు లేదా పని అని పిలవవచ్చు, లేదా దాచవచ్చు. దాచడం దాని 50 m లోపల ఉన్నదంతా మ్యాప్ బయట ఉంచుతుంది, మరియు మీరు ప్రతి రోజు ఎక్కడ ఉన్నారో మీ జర్నల్ రాసేది దాన్ని మినహాయిస్తుంది. ఇక్కడి రోజు జాబితా దాన్ని ఇంకా చూపిస్తుంది.

journal.window.map.visible.text.your.journal.couldn.t.read.the = ఈ రోజు ఉంచిన లొకేషన్‌ను మీ జర్నల్ చదవలేకపోయింది.

journal.window.map.visible.text.your.journal.draws.this.from.the = మీ జర్నల్ దీన్ని మీ ఫోన్ పంపిన కొలతల నుండి గీస్తుంది, మీ జర్నల్‌తో వచ్చిన పేర్ల నుండి, లేదా మీరు డౌన్‌లోడ్ చేసిన పేర్ల నుండి. ఏ మ్యాప్ సేవనూ ఏమీ అడగదు.

journal.window.map.visible.text.your.phone.often.reports.an.arrival = మీ ఫోన్ తరచు ముందు రాకను, తర్వాత సందర్శన మొత్తాన్ని నివేదిస్తుంది. రెండు నివేదికలు ఒక సందర్శన.

journal.window.map.week.heading = { $date }{ $slot1 }వారం

journal.window.map.week.readings.without.stops = { $quantity ->
    [one] { $count } కొలత, సందర్శనలు లేదా ఆగిన చోట్లు లేవు
   *[other] { $count } కొలతలు, సందర్శనలు లేదా ఆగిన చోట్లు లేవు
}

journal.window.map.window.explanation.a.place.in.your.journal = మీ జర్నల్‌లో ఒక ప్రదేశం

journal.window.map.window.explanation.a.segment.your.journal.couldn.t = మీ జర్నల్ చదవలేకపోయిన సెగ్మెంట్

journal.window.map.window.explanation.a.stay.your.journal.worked.out = మీ జర్నల్ తేల్చిన ఆగిన చోటు

journal.window.map.window.explanation.a.visit.your.phone.reported = మీ ఫోన్ నివేదించిన సందర్శన

journal.window.map.window.explanation.approximate.location.each.reading.is.only = సుమారు లొకేషన్: ప్రతి కొలత కొన్ని కిలోమీటర్‌ల లోపల మాత్రమే సరిపోతుంది, పట్టణానికి పేరు పెట్టడానికి చాలు, భవనానికి కాదు.

journal.window.map.window.explanation.choose.a.day.in.the.calendar = క్యాలెండర్‌లో ఒక రోజు ఎంచుకోండి.

journal.window.map.window.explanation.choose.an.area.on.the.map = మ్యాప్‌పై లేదా జాబితాలో ఒక ప్రాంతం ఎంచుకోండి, లేదా క్యాలెండర్‌లో ఒక రోజు.

journal.window.map.window.explanation.each.bar.is.a.visit.your = ప్రతి బార్ మీ ఫోన్ నివేదించిన సందర్శన.

journal.window.map.window.explanation.hidden.from.the.map = మ్యాప్ నుండి దాచబడింది

journal.window.map.window.explanation.in.the.chosen.area = ఎంచుకున్న ప్రాంతంలో

journal.window.map.window.explanation.location.accuracy.couldn.t.be.downloaded = లొకేషన్ ఖచ్చితత్వం డౌన్‌లోడ్ చేయలేకపోయింది. కాసేపటి తర్వాత మళ్ళీ ప్రయత్నించండి.

journal.window.map.window.explanation.location.accuracy.couldn.t.be.saved = లొకేషన్ ఖచ్చితత్వం మీ జర్నల్ ఫోల్డర్‌లో సేవ్ చేయలేకపోయింది.

journal.window.map.window.explanation.location.accuracy.failed.its.file.check = లొకేషన్ ఖచ్చితత్వం ఫైల్ సరిచూపు దాటలేదు. దాన్ని వాడే ముందు మళ్ళీ డౌన్‌లోడ్ చేయండి.

journal.window.map.window.explanation.location.comes.from.the.solstone.app = లొకేషన్ మీ ఫోన్‌లోని solstone యాప్ నుండి వస్తుంది, మీరు అక్కడ దాన్ని ఆన్ చేసిన తర్వాత. అది స్వీకరించేదంతా మీ జర్నల్‌లోకి వెళ్తుంది, రోజు రోజుకు ఇక్కడ కనిపిస్తుంది.

journal.window.map.window.explanation.location.couldn.t.be.read = లొకేషన్ చదవలేకపోయింది

journal.window.map.window.explanation.map.fitted.to.these.areas = ఈ ప్రాంతాలకు సరిపోయేలా అమర్చిన మ్యాప్

journal.window.map.window.explanation.margin.of.error.over.m.or = దోష పరిధి 500 m కంటే ఎక్కువ లేదా నివేదించబడలేదు

journal.window.map.window.explanation.no.named.town.nearby = దగ్గరలో పేరున్న పట్టణం లేదు

journal.window.map.window.explanation.open.it.in.transcripts.to.see = దాన్ని చూడటానికి లేదా తొలగించడానికి ట్రాన్స్క్రిప్ట్‌లలో తెరవండి

journal.window.map.window.explanation.the.area.with.the.most.days = అత్యధిక రోజులున్న ప్రాంతం

journal.window.map.window.explanation.this.trip.s.busiest.area = ఈ ప్రయాణంలో అత్యధికంగా ఉన్న ప్రాంతం

journal.window.map.window.explanation.this.version.of.your.journal.has = మీ జర్నల్ ఈ వెర్షన్‌లో లొకేషన్ ఖచ్చితత్వం డౌన్‌లోడ్ లేదు.

journal.window.map.window.explanation.tied.for.the.most.days = అత్యధిక రోజులకు సమానం

journal.window.map.window.explanation.where.two.reports.cover.the.same = రెండు నివేదికలు ఒకే సమయాన్ని కప్పే చోట, గంటలు ఆ సమయాన్ని ఒకసారి లెక్కిస్తాయి.

journal.window.map.window.explanation.your.journal.couldn.t.read.part = మీ జర్నల్ ఈ రోజు లొకేషన్‌లోని కొంత భాగాన్ని చదవలేకపోయింది.

journal.window.map.window.explanation.your.watch.gives.only.approximate.location = మీ వాచ్ సుమారు లొకేషన్ మాత్రమే ఇస్తుంది, కాబట్టి దాని కొలతలు పట్టణాన్ని పేరు పెడతాయి, భవనాన్ని కాదు.
