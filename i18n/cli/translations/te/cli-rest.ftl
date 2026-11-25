journal.cli.backfill.committed = కమిట్ అయింది

journal.cli.backfill.day.missing = రోజు { $day } జర్నల్‌లో కనబడలేదు

journal.cli.backfill.days = క్రానికల్ రోజులు జాబితా చేయలేకపోయింది: { $error }

journal.cli.backfill.flags.conflict = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: లోపం: ఆర్గ్యుమెంట్ { $flag }: ఆర్గ్యుమెంట్ { $previous } తో అనుమతి లేదు

journal.cli.backfill.flags.missing = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: లోపం: ఆర్గ్యుమెంట్ { $day_flag }: ఒక ఆర్గ్యుమెంట్ అవసరం

journal.cli.backfill.flags.unknown = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $argument }

journal.cli.backfill.help = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }ఖాళీ ప్రాసెసింగ్ నమోదులను ఇరుక్కున్న హెడర్-మాత్రమే స్థానిక{ $line_break }{ $handlers } అవుట్‌పుట్‌లపై బ్యాక్‌ఫిల్ చేయండి{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }ఈ సహాయ సందేశం చూపించి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $day_option }{ $gap_6 }రోజు { $date_format } ఫార్మాట్‌లో; డిఫాల్ట్‌గా అన్ని రోజులు{ $line_break }{ $indent_2 }{ $commit_option }{ $gap_7 }అర్హమైన { $jsonl_format } హెడర్‌లను ఖాళీ ప్రాసెసింగ్ నమోదులతో మళ్ళీ రాయండి{ $line_break }{ $indent_2 }{ $dry_run_option }{ $gap_6 }మార్పులు రాయకుండా అర్హమైన అవుట్‌పుట్‌ల ప్రివ్యూ చూడండి{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }వివరమైన అవుట్‌పుట్ ఎనేబుల్ చేయండి{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }డీబగ్ లాగింగ్ ఎనేబుల్ చేయండి

journal.cli.backfill.identity.ambiguous = పేరు "{ $default_stream }" ఉన్న ప్రవాహ డైరెక్టరీ నమోదు గుర్తింపుగా రాయలేం: { $path }

journal.cli.backfill.identity.duplicate = ఒకటి కంటే ఎక్కువ సెగ్మెంట్‌లు ప్రవాహం { $stream } కీ { $key } పంచుకుంటున్నాయి

journal.cli.backfill.identity.encoding = సెగ్మెంట్ పాత్ { $encoding } రూపంలో సూచించలేం: { $path }

journal.cli.backfill.invalid.day = { $date_format } ఫార్మాట్‌లో రోజు అవసరం

journal.cli.backfill.marker = { $path } స్టాంప్ అయింది, కానీ రోజు { $day } అప్‌డేట్ అయినట్టు గుర్తించలేకపోయింది: { $error }

journal.cli.backfill.preview = డ్రై-రన్ (మార్పులు రాయలేదు)

journal.cli.backfill.segment.directory = సెగ్మెంట్ డైరెక్టరీ { $path } జాబితా చేయలేకపోయింది: { $error }

journal.cli.backfill.segments = రోజు { $day } కోసం సెగ్మెంట్‌లు జాబితా చేయలేకపోయింది: { $error }

journal.cli.backfill.stamp.changed = { $path } స్టాంప్ చేయలేకపోయింది: వర్గీకరణ తర్వాత ఫైల్ మారింది

journal.cli.backfill.stamp.read = { $path } స్టాంప్ చేయలేకపోయింది: ఫైల్ మళ్ళీ చదవలేకపోయింది: { $error }

journal.cli.backfill.stamp.write = { $path } స్టాంప్ చేయలేకపోయింది: రాయడం విఫలమైంది: { $error }

journal.cli.backfill.stream.directory = ప్రవాహ డైరెక్టరీ { $path } జాబితా చేయలేకపోయింది: డైరెక్టరీ కాదు

journal.cli.backfill.stream.encoding = సెగ్మెంట్ ప్రవాహం { $encoding } రూపంలో సూచించలేం: { $path }

journal.cli.brain.arguments.invalid = { $command }: లోపం: చెల్లని ఆర్గ్యుమెంట్‌లు

journal.cli.brain.busy = ఆలోచన బిజీగా ఉంది: ఒక సరిచూపు ఇప్పటికే నడుస్తోంది

journal.cli.brain.check.config = ఆలోచన సరిచూపు విఫలమైంది: జర్నల్ కాన్ఫిగ్ చదవలేకపోయింది: { $detail }

journal.cli.brain.check.failure = ఆలోచన సరిచూపు విఫలమైంది: { $detail }

journal.cli.brain.check.fingerprint.options = ఊహించిన యాక్టివ్ ఫింగర్‌ప్రింట్ మరియు ఊహించిన లేకపోవడం రెండూ కలిసి ఉండవు

journal.cli.brain.check.key.missing = ప్రొవైడర్‌కు కీ అవసరం, మరియు మీ జర్నల్ ఫలితం సేవ్ అయిందని నిర్ధారించలేకపోయింది.

journal.cli.brain.check.key.rejected = ప్రొవైడర్ ఈ కీని నిరాకరించింది, మరియు మీ జర్నల్ ఫలితం సేవ్ అయిందని నిర్ధారించలేకపోయింది.

journal.cli.brain.check.open.directory = { $path } వద్ద ఫ్లాట్-డైరెక్టరీ లోపలి పాత్ తెరవడం విఫలమైంది: { $reason }

journal.cli.brain.check.random = సురక్షిత యాదృచ్ఛిక బైట్‌లు రూపొందించలేకపోయింది

journal.cli.brain.heading.blocked = ఆలోచన బ్లాక్ అయింది: { $reason }

journal.cli.brain.heading.blocked.component = ఆలోచన బ్లాక్ అయింది: { $reason } ({ $component })

journal.cli.brain.heading.checking = ఆలోచన సరిచూస్తోంది: { $reason }

journal.cli.brain.heading.checking.component = ఆలోచన సరిచూస్తోంది: { $reason } ({ $component })

journal.cli.brain.heading.other = ఆలోచన { $state }: { $reason }

journal.cli.brain.heading.other.component = ఆలోచన { $state }: { $reason } ({ $component })

journal.cli.brain.heading.unhealthy = ఆలోచన సరిగా లేదు: { $reason }

journal.cli.brain.heading.unhealthy.component = ఆలోచన సరిగా లేదు: { $reason } ({ $component })

journal.cli.brain.heading.unknown = ఆలోచన తెలియదు: { $reason }

journal.cli.brain.heading.unknown.component = ఆలోచన తెలియదు: { $reason } ({ $component })

journal.cli.brain.help.refresh = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }ఈ సహాయ సందేశం చూపించి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }{ $json_format } సాదా అవుట్‌పుట్ బదులు ఇవ్వండి{ $line_break }{ $syntax_continuation_7 }{ $line_break }{ $syntax_continuation_8 }{ $line_break }{ $syntax_continuation_9 }

journal.cli.brain.help.renew = usage: { $syntax }{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }ఈ సహాయ సందేశం చూపించి నిష్క్రమించండి

journal.cli.brain.help.root = usage: { $syntax }{ $line_break }{ $line_break }యాక్టివ్ ఆలోచన స్థితి మరియు పరిమిత రిఫ్రెష్ CLI.{ $line_break }{ $line_break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $status }యాక్టివ్ ఆలోచన స్థితి చూపండి{ $line_break }    { $refresh }ఒక పరిమిత యాక్టివ్ ఆలోచన సరిచూపు రన్ చేయండి{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_10 }ఈ సహాయ సందేశం చూపించి నిష్క్రమించండి

journal.cli.brain.help.status = usage: { $syntax }{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }ఈ సహాయ సందేశం చూపించి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }{ $json_format } సాదా అవుట్‌పుట్ బదులు ఇవ్వండి

journal.cli.brain.hold.capacity = క్రెడెన్షియల్ హోల్డ్ కెపాసిటీ అందుబాటులో లేదు

journal.cli.brain.hold.invalid.provider = మద్దతు లేని క్రెడెన్షియల్ హోల్డ్ ప్రొవైడర్

journal.cli.brain.hold.root.changed = అంగీకరించబడిన జర్నల్ రూట్ మారింది

journal.cli.brain.hold.superseded = క్రెడెన్షియల్ సరిచూపు సూపర్‌సీడ్ అయింది

journal.cli.brain.hold.unavailable = క్రెడెన్షియల్ హోల్డ్ స్థితి అందుబాటులో లేదు

journal.cli.brain.ready.byo = ఆలోచన సిద్ధంగా ఉంది: మీ సొంత మోడల్

journal.cli.brain.ready.byo.checked = ఆలోచన సిద్ధంగా ఉంది: మీ సొంత మోడల్, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.byo.model = ఆలోచన సిద్ధంగా ఉంది: మీ సొంత మోడల్ { $model }

journal.cli.brain.ready.byo.model.checked = ఆలోచన సిద్ధంగా ఉంది: మీ సొంత మోడల్ { $model }, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.local = ఆలోచన సిద్ధంగా ఉంది: లోకల్ ఆలోచన

journal.cli.brain.ready.local.checked = ఆలోచన సిద్ధంగా ఉంది: లోకల్ ఆలోచన, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.local.model = ఆలోచన సిద్ధంగా ఉంది: లోకల్ ఆలోచన { $model }

journal.cli.brain.ready.local.model.checked = ఆలోచన సిద్ధంగా ఉంది: లోకల్ ఆలోచన { $model }, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.other = ఆలోచన సిద్ధంగా ఉంది: { $lane }

journal.cli.brain.ready.other.checked = ఆలోచన సిద్ధంగా ఉంది: { $lane }, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.other.model = ఆలోచన సిద్ధంగా ఉంది: { $lane } { $model }

journal.cli.brain.ready.other.model.checked = ఆలోచన సిద్ధంగా ఉంది: { $lane } { $model }, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.spp = ఆలోచన సిద్ధంగా ఉంది: కాన్ఫిడెన్షియల్ ప్రాసెసింగ్

journal.cli.brain.ready.spp.checked = ఆలోచన సిద్ధంగా ఉంది: కాన్ఫిడెన్షియల్ ప్రాసెసింగ్, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.spp.model = ఆలోచన సిద్ధంగా ఉంది: కాన్ఫిడెన్షియల్ ప్రాసెసింగ్ { $model }

journal.cli.brain.ready.spp.model.checked = ఆలోచన సిద్ధంగా ఉంది: కాన్ఫిడెన్షియల్ ప్రాసెసింగ్ { $model }, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.unknown = ఆలోచన సిద్ధంగా ఉంది: తెలియదు

journal.cli.brain.ready.unknown.checked = ఆలోచన సిద్ధంగా ఉంది: తెలియదు, { $age } క్రితం సరిచూసింది

journal.cli.brain.ready.unknown.model = ఆలోచన సిద్ధంగా ఉంది: తెలియదు { $model }

journal.cli.brain.ready.unknown.model.checked = ఆలోచన సిద్ధంగా ఉంది: తెలియదు { $model }, { $age } క్రితం సరిచూసింది

journal.cli.brain.settlement.fingerprint.changed = రక్షిత ఫింగర్‌ప్రింట్ కీ సరిచూపు తర్వాత మారింది

journal.cli.brain.settlement.publication.changed = క్రెడెన్షియల్ సరిచూపు ప్రచురణ తర్వాత మారింది

journal.cli.brain.settlement.root.changed = అంగీకరించబడిన జర్నల్ పాత్ మారింది

journal.cli.brain.settlement.selection.changed = రిఫ్రెష్ సమయంలో క్రెడెన్షియల్ ఎంపిక మారింది

journal.cli.brain.settlement.unconfirmed = క్రెడెన్షియల్ సరిచూపు ప్రచురణ నిర్ధారణ కాలేదు

journal.cli.brain.settlement.validation.changed = రిఫ్రెష్ తర్వాత క్రెడెన్షియల్ సరిచూపు మారింది

journal.cli.brain.usage = usage: { $syntax }

journal.cli.brain.validation.checking.aggregate.requires.matching.checking.marker = సరిచూపు అగ్రిగేట్‌కు సరిపోయే సరిచూపు గుర్తు అవసరం

journal.cli.brain.validation.checking.reason.is.not.valid.component.evidence = సరిచూపు కారణం చెల్లుబాటు అయ్యే కాంపోనెంట్ ఆధారం కాదు

journal.cli.brain.validation.component.status.does.not.match.reason.aggregate = కాంపోనెంట్ స్థితి కారణ అగ్రిగేట్‌తో సరిపోలేదు

journal.cli.brain.validation.detail = { $field }: { $reason }

journal.cli.brain.validation.diagnostic.enum.value.not.allowed = డయాగ్నస్టిక్ ఎనమ్ విలువకు అనుమతి లేదు

journal.cli.brain.validation.diagnostic.key.not.allowed = డయాగ్నస్టిక్ కీకు అనుమతి లేదు

journal.cli.brain.validation.diagnostic.value.must.be.an.enum.string = డయాగ్నస్టిక్ విలువ ఎనమ్ స్ట్రింగ్ అయి ఉండాలి

journal.cli.brain.validation.does.not.match.aggregate.state = { $token_aggregate_state } తో సరిపోలేదు

journal.cli.brain.validation.expected.sha256.hex.string = { $token_sha_256 } హెక్స్ స్ట్రింగ్ అవసరం

journal.cli.brain.validation.is.required.when.status.is.ok = స్థితి ok అయి ఉన్నప్పుడు అవసరం

journal.cli.brain.validation.missing.lane.applicable.evidence.without.higher.priority.cause = ఎక్కువ ప్రాధాన్యం ఉన్న కారణం లేకుండా లేన్‌కు వర్తించే ఆధారం లేదు

journal.cli.brain.validation.must.be.a.non.negative.integer = రుణాత్మకం కాని పూర్ణాంకం అయి ఉండాలి

journal.cli.brain.validation.must.be.a.sha256.hex.digest = { $token_sha_256 } హెక్స్ digest అయి ఉండాలి

journal.cli.brain.validation.must.be.a.string = స్ట్రింగ్ అయి ఉండాలి

journal.cli.brain.validation.must.be.a.string.or.null = స్ట్రింగ్ లేదా null అయి ఉండాలి

journal.cli.brain.validation.must.be.an.integer = పూర్ణాంకం అయి ఉండాలి

journal.cli.brain.validation.must.be.an.iso8601.timestamp = { $token_iso_8601 } టైమ్‌స్టాంప్ అయి ఉండాలి

journal.cli.brain.validation.must.be.an.object = ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.brain.validation.must.be.evidence.recordable = ఆధారంగా నమోదు చేయదగినది అయి ఉండాలి

journal.cli.brain.validation.must.be.non.empty = ఖాళీ కానిది అయి ఉండాలి

journal.cli.brain.validation.must.be.null.when.aggregate.state.is.ready = { $token_aggregate_state } సిద్ధంగా ఉన్నప్పుడు null అయి ఉండాలి

journal.cli.brain.validation.must.be.provider.key.invalid = { $token_provider_key_invalid } అయి ఉండాలి

journal.cli.brain.validation.must.be.set.when.aggregate.state.is.not.ready = { $token_aggregate_state } సిద్ధంగా లేనప్పుడు సెట్ అయి ఉండాలి

journal.cli.brain.validation.non.ok.evidence.requires.reason = ok కాని ఆధారానికి కారణం అవసరం

journal.cli.brain.validation.not.attempted.is.only.valid.for.generate = { $token_not_attempted } generate కోసం మాత్రమే చెల్లుతుంది

journal.cli.brain.validation.not.attempted.reason.must.repeat.causal.prerequisite.reason = { $token_not_attempted } కారణం కారణాత్మక ముందస్తు షరతు కారణాన్ని మళ్ళీ చెప్పాలి

journal.cli.brain.validation.not.attempted.requires.a.non.ok.prerequisite = { $token_not_attempted } కోసం ok కాని ముందస్తు షరతు అవసరం

journal.cli.brain.validation.ok.evidence.requires.null.reason = ok ఆధారానికి null కారణం అవసరం

journal.cli.brain.validation.prerequisite.renewal.requires.ok.or.declared.failure = ముందస్తు షరతు రెన్యూవల్‌కు ok లేదా ప్రకటించిన వైఫల్యం అవసరం

journal.cli.brain.validation.reason.not.allowed.for.evidence.component = ఆధార కాంపోనెంట్ కోసం కారణానికి అనుమతి లేదు

journal.cli.brain.validation.record.aggregate.reason.does.not.match.evidence = నమోదు అగ్రిగేట్/కారణం ఆధారంతో సరిపోలేదు

journal.cli.brain.validation.unknown.field = తెలియని ఫీల్డ్

journal.cli.brain.validation.unknown.top.level.field = తెలియని పై స్థాయి ఫీల్డ్

journal.cli.brain.validation.unknown.value = తెలియని విలువ

journal.cli.brain.validation.unsupported.binding.version = మద్దతు లేని బైండింగ్ వెర్షన్

journal.cli.brain.validation.unsupported.schema.version = మద్దతు లేని స్కీమా వెర్షన్

journal.cli.brain.writer.active.brain.fingerprint.is.unavailable = యాక్టివ్ ఆలోచన ఫింగర్‌ప్రింట్ అందుబాటులో లేదు

journal.cli.brain.writer.brain.abandon.reason.is.not.recordable.evidence = ఆలోచనను విడిచిపెట్టిన కారణం నమోదు చేయదగిన ఆధారం కాదు

journal.cli.brain.writer.brain.fingerprint.changed = ఆలోచన ఫింగర్‌ప్రింట్ మారింది

journal.cli.brain.writer.brain.fingerprint.key.is.unavailable = ఆలోచన ఫింగర్‌ప్రింట్ కీ అందుబాటులో లేదు

journal.cli.brain.writer.brain.lane.changed = ఆలోచన లేన్ మారింది

journal.cli.brain.writer.brain.prerequisite.evidence.is.unsafe = ఆలోచన ముందస్తు షరతు ఆధారం సురక్షితం కాదు

journal.cli.brain.writer.brain.record.changed.during.inspection = పరిశీలించడం సమయంలో ఆలోచన నమోదు మారింది

journal.cli.brain.writer.brain.refresh.checking.marker.is.absent = ఆలోచన రిఫ్రెష్ సరిచూపు గుర్తు లేదు

journal.cli.brain.writer.brain.refresh.permit.expired = ఆలోచన రిఫ్రెష్ పర్మిట్ గడువు అయింది

journal.cli.brain.writer.brain.refresh.record.revision.changed = ఆలోచన రిఫ్రెష్ నమోదు రివిజన్ మారింది

journal.cli.brain.writer.brain.refresh.revision.changed = ఆలోచన రిఫ్రెష్ రివిజన్ మారింది

journal.cli.brain.writer.brain.refresh.run.id.changed = ఆలోచన రిఫ్రెష్ రన్ ఐడి మారింది

journal.cli.brain.writer.brain.runtime.failure.marker.changed = ఆలోచన రన్‌టైమ్ వైఫల్య గుర్తు మారింది

journal.cli.brain.writer.configuration.rejection.changed.after.publication = ప్రచురణ తర్వాత కాన్ఫిగ్ నిరాకరణ మారింది

journal.cli.brain.writer.credential.authority.changed.during.inspection = పరిశీలించడం సమయంలో క్రెడెన్షియల్ అథారిటీ మారింది

journal.cli.brain.writer.credential.authority.changed.during.observation = పరిశీలన సమయంలో క్రెడెన్షియల్ అథారిటీ మారింది

journal.cli.brain.writer.credential.authority.is.not.an.object = క్రెడెన్షియల్ అథారిటీ ఆబ్జెక్ట్ కాదు

journal.cli.brain.writer.credential.authority.is.unreadable = క్రెడెన్షియల్ అథారిటీ చదవలేం

journal.cli.brain.writer.credential.binding.authority.could.not.be.read.error = క్రెడెన్షియల్ బైండింగ్ అథారిటీ చదవలేకపోయింది: { $error }

journal.cli.brain.writer.credential.hold.authority.changed = క్రెడెన్షియల్ హోల్డ్ అథారిటీ మారింది

journal.cli.brain.writer.credential.hold.authority.is.not.an.object = క్రెడెన్షియల్ హోల్డ్ అథారిటీ ఆబ్జెక్ట్ కాదు

journal.cli.brain.writer.credential.hold.authority.is.unreadable.error = క్రెడెన్షియల్ హోల్డ్ అథారిటీ చదవలేం: { $error }

journal.cli.brain.writer.credential.observation.authority.is.not.an.object = క్రెడెన్షియల్ పరిశీలన అథారిటీ ఆబ్జెక్ట్ కాదు

journal.cli.brain.writer.credential.observation.authority.is.unreadable = క్రెడెన్షియల్ పరిశీలన అథారిటీ చదవలేం

journal.cli.brain.writer.credential.observation.changed.during.refresh = రిఫ్రెష్ సమయంలో క్రెడెన్షియల్ పరిశీలన మారింది

journal.cli.brain.writer.credential.observation.changed.during.work = పని సమయంలో క్రెడెన్షియల్ పరిశీలన మారింది

journal.cli.brain.writer.credential.observation.identity.is.invalid = క్రెడెన్షియల్ పరిశీలన గుర్తింపు చెల్లదు

journal.cli.brain.writer.credential.observation.lacks.original.binding = క్రెడెన్షియల్ పరిశీలనకు అసలు బైండింగ్ లేదు

journal.cli.brain.writer.credential.observations.are.not.an.object = క్రెడెన్షియల్ పరిశీలనలు ఆబ్జెక్ట్ కావు

journal.cli.brain.writer.credential.process.state.changed.during.inspection = పరిశీలించడం సమయంలో క్రెడెన్షియల్ ప్రాసెస్ స్థితి మారింది

journal.cli.brain.writer.credential.publication.reservation.is.absent = క్రెడెన్షియల్ ప్రచురణ రిజర్వేషన్ లేదు

journal.cli.brain.writer.credential.rejection.changed.after.publication = ప్రచురణ తర్వాత క్రెడెన్షియల్ నిరాకరణ మారింది

journal.cli.brain.writer.credential.rejection.changed.during.refresh = రిఫ్రెష్ సమయంలో క్రెడెన్షియల్ నిరాకరణ మారింది

journal.cli.brain.writer.credential.rejection.changed.during.validation = సరిచూపు సమయంలో క్రెడెన్షియల్ నిరాకరణ మారింది

journal.cli.brain.writer.field.must.be.an.object = { $field } ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.brain.writer.protected.fingerprint.key.changed.after.rejection = నిరాకరణ తర్వాత రక్షిత ఫింగర్‌ప్రింట్ కీ మారింది

journal.cli.brain.writer.protected.fingerprint.key.changed.during.admission = అంగీకారం సమయంలో రక్షిత ఫింగర్‌ప్రింట్ కీ మారింది

journal.cli.brain.writer.protected.fingerprint.key.changed.during.inspection = పరిశీలించడం సమయంలో రక్షిత ఫింగర్‌ప్రింట్ కీ మారింది

journal.cli.brain.writer.protected.fingerprint.key.changed.during.validation = సరిచూపు సమయంలో రక్షిత ఫింగర్‌ప్రింట్ కీ మారింది

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.a.credential.binding.exists = క్రెడెన్షియల్ బైండింగ్ ఉన్నప్పుడు రక్షిత ఫింగర్‌ప్రింట్ కీ లేదు

journal.cli.brain.writer.protected.fingerprint.key.is.absent.while.credential.authority.exists = క్రెడెన్షియల్ అథారిటీ ఉన్నప్పుడు రక్షిత ఫింగర్‌ప్రింట్ కీ లేదు

journal.cli.brain.writer.protected.fingerprint.key.is.unavailable = రక్షిత ఫింగర్‌ప్రింట్ కీ అందుబాటులో లేదు

journal.cli.brain.writer.reason.is.not.recordable.evidence = కారణం నమోదు చేయదగిన ఆధారం కాదు

journal.cli.brain.writer.rejected.refresh.receipt.does.not.match.its.probe = నిరాకరించిన రిఫ్రెష్ రసీదు దాని ప్రోబ్‌తో సరిపోలేదు

journal.cli.brain.writer.rejection.receipt.root.provider.changed = నిరాకరణ రసీదు రూట్/ప్రొవైడర్ మారింది

journal.cli.brain.writer.settled.credential.observation.changed.during.work = పని సమయంలో సెటిల్ అయిన క్రెడెన్షియల్ పరిశీలన మారింది

journal.cli.brain.writer.validation.result.does.not.match.its.credential = సరిచూపు ఫలితం దాని క్రెడెన్షియల్‌తో సరిపోలేదు

journal.cli.brain.writer.validation.result.is.not.an.object = సరిచూపు ఫలితం ఆబ్జెక్ట్ కాదు

journal.cli.depict.admission = జర్నల్ ప్రాసెస్ అంగీకారం విఫలమైంది: { $detail }

journal.cli.depict.admission.binding.mismatch = లాంచ్ ఇన్‌స్టాలేషన్ బైండింగ్ సరిపోలలేదు

journal.cli.depict.admission.birth.invalid = { $linux } ప్రాసెస్ ప్రారంభంలో { $boot } ఆకృతి తప్పు

journal.cli.depict.admission.birth.position = { $linux } ప్రాసెస్ ప్రారంభంలో { $boot } ఆకృతి తప్పు, పంక్తి { $line } నిలువు { $column } వద్ద

journal.cli.depict.admission.byte.count = లాంచ్ పైప్ చెల్లని బైట్ సంఖ్య ఇచ్చింది

journal.cli.depict.admission.cleanup.pending = మునుపటి లాంచ్ { $io } క్లీనప్ ఇంకా పూర్తి కాలేదు

journal.cli.depict.admission.commit.mismatch = లాంచ్ అంగీకార కమిట్ సరిపోలలేదు

journal.cli.depict.admission.deadline.elapsed = లాంచ్ గడువు దాటింది

journal.cli.depict.admission.deadline.invalid = గడువు అయిన లేదా చెల్లని { $windows } లాంచ్ గడువు

journal.cli.depict.admission.descriptor.encoding = { $unicode } కాని { $windows } లాంచ్ డిస్క్రిప్టర్

journal.cli.depict.admission.descriptor.size = పరిమాణం మించిన { $windows } లాంచ్ డిస్క్రిప్టర్

journal.cli.depict.admission.endpoint = చెల్లని లాంచ్ endpoint

journal.cli.depict.admission.entry.hosted = హోస్ట్ చేసిన ఎంట్రీ వద్ద ఇన్‌స్టాల్ చేసిన పని

journal.cli.depict.admission.entry.mismatch = లాంచ్ వేరియంట్ లేదా ఇన్‌స్టాల్ చేసిన చర్య సరిపోలలేదు

journal.cli.depict.admission.frame.size = చెల్లని లాంచ్ ఫ్రేమ్ పరిమాణం

journal.cli.depict.admission.grant.access = లాంచ్ హ్యాండిల్ యాక్సెస్ లేదా టైప్ దాని అనుమతితో సరిపోలేదు

journal.cli.depict.admission.grant.disk = లాంచ్ అనుమతి డిస్క్ ఫైల్ కాదు

journal.cli.depict.admission.grant.duplicate = నకలు లాంచ్ హ్యాండిల్ లేదా అనుమతి రకం

journal.cli.depict.admission.grant.ordinary = లాంచ్ అనుమతి సాధారణ డిస్క్ ఫైల్ కాదు

journal.cli.depict.admission.guard.duplicate = ఇన్‌స్టాలేషన్ గార్డ్: నకలు గుర్తింపు గార్డ్ { $field }

journal.cli.depict.admission.guard.encoding = { $unicode } కాని ఇన్‌స్టాలేషన్ గార్డ్

journal.cli.depict.admission.guard.invalid = ఇన్‌స్టాలేషన్ గార్డ్: చెల్లని గుర్తింపు గార్డ్ { $field }

journal.cli.depict.admission.guard.mismatch = లాంచ్ గార్డ్ టపుల్ వారసత్వ గార్డ్‌కు భిన్నంగా ఉంది

journal.cli.depict.admission.guard.missing = ఇన్‌స్టాలేషన్ గార్డ్: గుర్తింపు గార్డ్ ఫీల్డ్‌లు పూర్తి కాలేదు

journal.cli.depict.admission.handle.invalid = చెల్లని బదిలీ హ్యాండిల్

journal.cli.depict.admission.identity.io.local.folder = { $local_data } తెలిసిన ఫోల్డర్ రిజల్వ్: { $error }

journal.cli.depict.admission.identity.io.marker.clone = అడాప్షన్ గుర్తు క్లోన్: { $error }

journal.cli.depict.admission.identity.io.marker.read = అడాప్షన్ గుర్తు చదవడం: { $error }

journal.cli.depict.admission.identity.io.marker.stat = అడాప్షన్ గుర్తు stat: { $error }

journal.cli.depict.admission.identity.io.namespace.read = నేమ్‌స్పేస్ డైరెక్టరీ చదవడం: { $error }

journal.cli.depict.admission.identity.io.owner.open = ఓనర్ డైరెక్టరీ తెరవడం: { $error }

journal.cli.depict.admission.identity.io.record.read = గుర్తింపు నమోదు చదవడం: { $error }

journal.cli.depict.admission.identity.io.record.stat = గుర్తింపు నమోదు stat: { $error }

journal.cli.depict.admission.identity.io.root = రూట్ క్యానానికల్ చేయడం: { $error }

journal.cli.depict.admission.identity.io.storage.attrs = నిల్వ లక్షణాలు క్వెరీ: { $error }

journal.cli.depict.admission.identity.io.storage.file = నిల్వ ఫైల్ తెరవడం: { $error }

journal.cli.depict.admission.identity.io.storage.identity = నిల్వ గుర్తింపు క్వెరీ: { $error }

journal.cli.depict.admission.identity.io.storage.lock = గుర్తింపు నిల్వ లాక్: { $error }

journal.cli.depict.admission.identity.io.storage.open = నిల్వ డైరెక్టరీ తెరవడం: { $error }

journal.cli.depict.admission.identity.local.encoding = { $local_data } తెలిసిన ఫోల్డర్ చెల్లుబాటు అయ్యే { $utf } కాదు

journal.cli.depict.admission.identity.marker.content = అడాప్షన్ గుర్తులో చెల్లని కంటెంట్ ఉంది

journal.cli.depict.admission.identity.marker.missing = కమిట్ అయిన నమోదులో అడాప్షన్ గుర్తు లేదు

journal.cli.depict.admission.identity.marker.orphan = ఒంటరి అడాప్షన్ గుర్తు

journal.cli.depict.admission.identity.marker.size = అడాప్షన్ గుర్తు పరిమాణం చెల్లదు

journal.cli.depict.admission.identity.namespace.child = తెలియని నేమ్‌స్పేస్ చైల్డ్

journal.cli.depict.admission.identity.owner.poisoned = ఓనర్ కోఆర్డినేటర్ పాయిజన్ అయింది

journal.cli.depict.admission.identity.path.device = { $windows } వర్బాటిమ్ మరియు పరికర పాత్‌లు అంగీకరించబడవు

journal.cli.depict.admission.identity.path.encoding = { $windows } పాత్ చెల్లుబాటు అయ్యే { $utf } కాదు

journal.cli.depict.admission.identity.path.escape = { $windows } పాత్ తన అబ్సల్యూట్ రూట్‌ను దాటి వెళ్తుంది

journal.cli.depict.admission.identity.path.nul = { $windows } పాత్ మధ్యలో { $nul } కలిగి ఉంది

journal.cli.depict.admission.identity.path.relative = { $windows } పాత్ పూర్తిగా నిర్దేశించినది అయి ఉండాలి

journal.cli.depict.admission.identity.path.server = { $windows } { $unc } పాత్‌లో సర్వర్ లేదు

journal.cli.depict.admission.identity.path.share = { $windows } { $unc } పాత్‌లో షేర్ లేదు

journal.cli.depict.admission.identity.path.size = పాత్ { $limit } బైట్‌లు మించింది

journal.cli.depict.admission.identity.prepared = ఇన్‌స్టాలేషన్ గుర్తింపు నమోదు ప్రిపేర్ అయి ఉంది

journal.cli.depict.admission.identity.record.missing = నేమ్‌స్పేస్ నమోదు లేదు

journal.cli.depict.admission.identity.record.size = గుర్తింపు నమోదు పరిమితులు మించింది

journal.cli.depict.admission.identity.root.mismatch = నమోదు అడిగిన రూట్‌ను బైండ్ చేయదు

journal.cli.depict.admission.identity.storage.directory = నిల్వ పాత్ డైరెక్టరీ కాదు

journal.cli.depict.admission.identity.storage.file = నిల్వ పాత్ సాధారణ ఫైల్ కాదు

journal.cli.depict.admission.identity.storage.reparse = నిల్వ పాత్ రీపార్స్ పాయింట్

journal.cli.depict.admission.identity.tombstoned = ఇన్‌స్టాలేషన్ గుర్తింపు నమోదు Tombstone గా గుర్తించబడింది

journal.cli.depict.admission.nonce = చెల్లని లాంచ్ endpoint nonce

journal.cli.depict.admission.obsolete = పాతబడిన { $windows } లాంచ్ గుర్తు

journal.cli.depict.admission.offer.mismatch = లాంచ్ ఆఫర్ ప్రావెనెన్స్ సరిపోలలేదు

journal.cli.depict.admission.parent.mismatch = లాంచ్ సర్వర్ ఖచ్చితమైన ప్రత్యక్ష పేరెంట్ కాదు

journal.cli.depict.admission.parent.provenance = చెల్లని పేరెంట్ లాంచ్ ప్రావెనెన్స్

journal.cli.depict.admission.peer.unverified = లాంచ్ పైప్ పీర్ ధృవీకరించదగ్గ సజీవ స్థితిలో లేదు

journal.cli.depict.admission.process.creation = ప్రస్తుత { $windows } ప్రాసెస్ సృష్టి { $filetime } అందుబాటులో లేదు

journal.cli.depict.admission.process.signaled = ప్రస్తుత { $windows } ప్రాసెస్ సిగ్నల్ అయింది

journal.cli.depict.admission.provenance = చెల్లని లాంచ్ ప్రావెనెన్స్

journal.cli.depict.admission.record.ascii = గుర్తింపు నమోదు { $ascii } కాదు

journal.cli.depict.admission.record.checksum = గుర్తింపు నమోదు చెక్‌సమ్ సరిపోలలేదు

journal.cli.depict.admission.record.field = చెల్లని గుర్తింపు నమోదు { $field }

journal.cli.depict.admission.record.lines = గుర్తింపు నమోదు సరిగ్గా ఎనిమిది { $linefeed } పంక్తులు కాదు

journal.cli.depict.admission.record.size = గుర్తింపు నమోదు { $size } మించింది

journal.cli.depict.admission.root.missing = లాంచ్ ఎగ్జిక్యూటబుల్‌కు ఇన్‌స్టాలేషన్ రూట్ లేదు

journal.cli.depict.admission.server.mismatch = లాంచ్ పైప్ సర్వర్ గుర్తింపు సరిపోలలేదు

journal.cli.depict.admission.snapshot.absent = ప్రస్తుత ప్రాసెస్ పేరెంట్ స్నాప్‌షాట్‌లో లేదు

journal.cli.depict.admission.task.provenance = చెల్లని ఇన్‌స్టాల్ చేసిన పని ప్రావెనెన్స్

journal.cli.depict.admission.wait.complete = లాంచ్ { $io } వేచి ఉండటం విఫలమైంది: { $error }; { $io } రద్దు పూర్తయింది

journal.cli.depict.admission.wait.deadline.complete = లాంచ్ గడువు దాటింది; { $io } రద్దు పూర్తయింది

journal.cli.depict.admission.wait.deadline.incomplete = లాంచ్ గడువు దాటింది; { $io } క్లీనప్ పూర్తి కాలేదు మరియు నిలిపి ఉంచబడింది

journal.cli.depict.admission.wait.incomplete = లాంచ్ { $io } వేచి ఉండటం విఫలమైంది: { $error }; { $io } క్లీనప్ పూర్తి కాలేదు మరియు నిలిపి ఉంచబడింది

journal.cli.depict.detection.unavailable = వస్తువు గుర్తించడం అందుబాటులో లేదు. `{ $command }` రన్ చేయండి, { $engine } అసెట్‌లను సరిచూడటానికి లేదా బాగుచేయడానికి.

journal.cli.depict.detection.unavailable.windows =వస్తువు గుర్తించడం అందుబాటులో లేదు. మీ జర్నల్‌ను బాగుచేయండి లేదా మళ్ళీ ఇన్‌స్టాల్ చేయండి.

journal.cli.depict.help = usage: { $syntax }

journal.cli.depict.metadata.invalid = స్థానిక depict: చెల్లని { $variable } { $format }

journal.cli.describe.cancelled = describe రద్దు అయింది

journal.cli.describe.decode.failed = వీడియో డీకోడ్ చేయలేకపోయింది: { $path }

journal.cli.describe.deferred = describe వాయిదాలో ఉంది

journal.cli.describe.deferred.reason = describe వాయిదాలో ఉంది: { $reason }

journal.cli.describe.faces.skipped = ముఖ ఎన్‌రిచ్‌మెంట్ దాటవేయబడింది: { $kind }

journal.cli.describe.file.encoding = వీడియో పాత్‌కు { $encoding } ఫైల్ పేరు ఉండాలి

journal.cli.describe.output.encode = అవుట్‌పుట్ ఎన్‌కోడ్ చేయలేకపోయింది: { $error }

journal.cli.describe.output.write = అవుట్‌పుట్ రాయలేకపోయింది: { $error }

journal.cli.describe.path.config = జర్నల్ పాత్ చదవలేకపోయింది: { $reason }

journal.cli.describe.path.home = హోమ్ డైరెక్టరీ రిజల్వ్ చేయలేకపోయింది: { $reason }

journal.cli.describe.pipeline.flat.bound = బైండ్ అయిన డైరెక్టరీ పేరు ఉంది మరియు అది డైరెక్టరీ కాదు

journal.cli.describe.pipeline.flat.create.flat.directory.child = { $path } కోసం ఫ్లాట్ డైరెక్టరీ చైల్డ్ సృష్టించడం విఫలమైంది: { $detail }

journal.cli.describe.pipeline.flat.normal = పేరు ఒక నార్మల్ కాంపోనెంట్ అయి ఉండాలి

journal.cli.describe.pipeline.flat.read.flat.directory.identity = { $path } కోసం ఫ్లాట్ డైరెక్టరీ గుర్తింపు చదవడం విఫలమైంది: { $detail }

journal.cli.describe.pipeline.flat.relative = పాత్ ఖాళీ కాని నార్మల్ పోర్టబుల్ కాంపోనెంట్‌ల సీక్వెన్స్ అయి ఉండాలి

journal.cli.describe.pipeline.flat.revalidate.flat.directory = { $path } కోసం ఫ్లాట్ డైరెక్టరీని మళ్ళీ సరిచూడటం విఫలమైంది: { $detail }

journal.cli.describe.pipeline.flat.stat.flat.directory.descendant = { $path } కోసం ఫ్లాట్ డైరెక్టరీ డిసెండెంట్ stat విఫలమైంది: { $detail }

journal.cli.describe.pipeline.flat.stat.opened.flat.directory.descendant = { $path } కోసం ఫ్లాట్ డైరెక్టరీకి చెందిన తెరిచిన డిసెండెంట్ stat విఫలమైంది: { $detail }

journal.cli.describe.pipeline.flat.utf8 = పేరు చెల్లుబాటు అయ్యే { $encoding } వచనం కాదు

journal.cli.describe.pipeline.frames.failed = describe ఫ్రేమ్ అభ్యర్థనలన్నీ విఫలమయ్యాయి

journal.cli.describe.pipeline.install.observe.installed.destination.after.move = { $path }: తరలింపు తర్వాత ఇన్‌స్టాల్ అయిన గమ్యం అబ్జర్వ్ చేయడం: { $detail }

journal.cli.describe.pipeline.install.reconcile.install.after.failed.move = { $path }: విఫలమైన తరలింపు తర్వాత ఇన్‌స్టాల్ సమన్వయం చేయడం: { $detail }

journal.cli.describe.pipeline.install.revalidate.install.paths.after.move = { $path }: తరలింపు తర్వాత ఇన్‌స్టాల్ పాత్‌లు మళ్ళీ సరిచూడటం: { $detail }

journal.cli.describe.pipeline.jobs = { $flag } ధనాత్మకం అయి ఉండాలి

journal.cli.describe.pipeline.merge.frame = మెర్జ్ ఫ్రేమ్ లేదు

journal.cli.describe.pipeline.metadata.object = { $segment_meta } ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.describe.pipeline.ocr.coverage = స్క్రీన్ { $ocr } కవరేజ్ అసంపూర్ణం; ప్రైవేట్ ఫ్రేమ్ డయాగ్నస్టిక్స్ ఖాళీని నమోదు చేస్తాయి

journal.cli.describe.pipeline.ocr.publication = స్క్రీన్ { $ocr } ప్రచురణ విఫలమైంది: { $kind }

journal.cli.describe.pipeline.ocr.retire = స్క్రీన్ { $ocr } పాతబడిన అగ్రిగేట్‌ను రిటైర్ చేయలేకపోయింది: { $kind }

journal.cli.describe.pipeline.parent = వీడియోకు పేరెంట్ లేదు

journal.cli.describe.pipeline.progress = describe ఫ్రేమ్ విండో ప్రోగ్రెస్ చేయలేదు

journal.cli.describe.pipeline.publish.anchor.is.not.a.mount.manager.volume.guid.path = యాంకర్ { $manager } వాల్యూమ్ { $guid } పాత్ కాదు

journal.cli.describe.pipeline.publish.changed = సిద్ధం చేసిన ప్రచురణ మారింది

journal.cli.describe.pipeline.publish.component = ప్రచురణ లీఫ్ ప్లెయిన్ కాంపోనెంట్ కాదు

journal.cli.describe.pipeline.publish.current.directory.is.not.valid.utf = ప్రస్తుత డైరెక్టరీ చెల్లుబాటు అయ్యే { $encoding } కాదు

journal.cli.describe.pipeline.publish.destination = ప్రచురణ గమ్యం ప్లెయిన్ ఫైల్ కాదు

journal.cli.describe.pipeline.publish.destination.already.identifies.the.source.file = గమ్యం ఇప్పటికే మూల ఫైల్‌ను సూచిస్తుంది

journal.cli.describe.pipeline.publish.destination.changed.during.publication = ప్రచురణలో గమ్యం మారింది

journal.cli.describe.pipeline.publish.destination.has.no.normal.name = గమ్యానికి నార్మల్ పేరు లేదు

journal.cli.describe.pipeline.publish.destination.is.a.reparse.point = గమ్యం రిపార్స్ పాయింట్ అయి ఉంది

journal.cli.describe.pipeline.publish.destination.is.not.a.regular.file = గమ్యం సాధారణ ఫైల్ కాదు

journal.cli.describe.pipeline.publish.identity = సిద్ధం చేసిన ప్రచురణ గుర్తింపు మారింది

journal.cli.describe.pipeline.publish.identity.value.out.of.range = గుర్తింపు విలువ పరిధి బయట ఉంది

journal.cli.describe.pipeline.publish.install.publication.exhausted = ఇన్‌స్టాల్ ప్రచురణ అయిపోయింది

journal.cli.describe.pipeline.publish.leaf = ప్రచురణ లీఫ్ లేదు

journal.cli.describe.pipeline.publish.mode.exceeds.o = మోడ్ { $octal } దాటింది

journal.cli.describe.pipeline.publish.move.spelling.contains.an.interior.nul = తరలింపు స్పెల్లింగ్‌లో లోపల { $nul } ఉంది

journal.cli.describe.pipeline.publish.parent = ప్రచురణ పేరెంట్ లేదు

journal.cli.describe.pipeline.publish.path = { $path }: { $detail }

journal.cli.describe.pipeline.publish.publication.path.is.not.valid.utf = ప్రచురణ పాత్ చెల్లుబాటు అయ్యే { $encoding } కాదు

journal.cli.describe.pipeline.publish.published.destination.identity.does.not.match.the.live.source = ప్రచురించిన గమ్య గుర్తింపు లైవ్ మూలంతో సరిపోలేదు

journal.cli.describe.pipeline.publish.published.destination.is.not.a.regular.file = ప్రచురించిన గమ్యం సాధారణ ఫైల్ కాదు

journal.cli.describe.pipeline.publish.published.destination.name.is.absent = ప్రచురించిన గమ్యం పేరు లేదు

journal.cli.describe.pipeline.publish.source.and.destination.are.on.different.volumes = మూలం మరియు గమ్యం వేర్వేరు వాల్యూమ్‌లపై ఉన్నాయి

journal.cli.describe.pipeline.publish.source.and.destination.name.the.same.file = మూలం మరియు గమ్యం ఒకే ఫైల్‌కు పేరు పెడతాయి

journal.cli.describe.pipeline.publish.source.has.no.normal.name = మూలానికి నార్మల్ పేరు లేదు

journal.cli.describe.pipeline.publish.source.identity.changed.before.publication = ప్రచురణకు ముందు మూల గుర్తింపు మారింది

journal.cli.describe.pipeline.publish.source.identity.is.negative = మూల గుర్తింపు రుణాత్మకం

journal.cli.describe.pipeline.publish.source.name.changed.before.publication = ప్రచురణకు ముందు మూలం పేరు మారింది

journal.cli.describe.pipeline.publish.source.name.is.absent.before.publication = ప్రచురణకు ముందు మూలం పేరు లేదు

journal.cli.describe.pipeline.publish.sync = describe ప్రచురణ పేరెంట్ సింక్ విఫలమైంది: { $error }

journal.cli.describe.pipeline.publish.uncertain = { $path }: { $operation }: { $detail }

journal.cli.describe.pipeline.reappear = describe ఫ్రేమ్ { $frame } అదే టైమ్‌స్టాంప్‌తో మళ్ళీ కనిపించలేదు

journal.cli.describe.pipeline.record.parent.changed = స్క్రీన్ వచనం పేరెంట్ మారింది

journal.cli.describe.pipeline.record.parent.plain = స్క్రీన్ వచనం పేరెంట్ ప్లెయిన్ డైరెక్టరీ కాదు

journal.cli.describe.pipeline.reopen = ఫ్రేమ్ పేలోడ్‌లను మెటీరియలైజ్ చేయడానికి describe వీడియోను మళ్ళీ తెరవలేకపోయింది

journal.cli.describe.pipeline.resize = కేటగరైజేషన్ ఫ్రేమ్ పరిమాణం మార్చలేకపోయింది

journal.cli.describe.pipeline.response.id = సెషన్ రెస్పాన్స్ id వదిలేసింది

journal.cli.describe.pipeline.response.unmatched = సంబంధం లేని రెస్పాన్స్

journal.cli.describe.pipeline.retry.binding = { $ocr } మళ్ళీ ప్రయత్నం ఫ్రేమ్ బైండింగ్ మారింది

journal.cli.describe.pipeline.retry.evidence.missing = మళ్ళీ ప్రయత్నం ఆధారం లేదు

journal.cli.describe.pipeline.retry.handler = స్థానిక వైఫల్యం హ్యాండ్లర్ మారింది

journal.cli.describe.pipeline.retry.id = స్థానిక మళ్ళీ ప్రయత్నం గుర్తింపు { $encoding } కాదు

journal.cli.describe.pipeline.retry.incomplete = { $ocr } మళ్ళీ ప్రయత్నం చివరి అర్హత ఫ్రేమ్‌కు చేరలేదు

journal.cli.describe.pipeline.retry.input.changed = చదువుతుండగా మళ్ళీ ప్రయత్నం ఇన్‌పుట్ మారింది

journal.cli.describe.pipeline.retry.input.regular = మళ్ళీ ప్రయత్నం ఇన్‌పుట్ సాధారణ ఫైల్ కాదు

journal.cli.describe.pipeline.retry.io.allocate = స్టేజ్ కేటాయించలేకపోయింది

journal.cli.describe.pipeline.retry.io.dest.namespace = ప్రచురణకు ముందు గమ్యం నేమ్‌స్పేస్ మారింది

journal.cli.describe.pipeline.retry.io.file.kind = ఎంట్రీ సాధారణ ఫైల్ కాదు

journal.cli.describe.pipeline.retry.io.file.reparse = ఎంట్రీ రిపార్స్ పాయింట్ అయి ఉంది

journal.cli.describe.pipeline.retry.io.fsync = { $path } కోసం పేరెంట్ డైరెక్టరీ fsync బలహీనంగా ఉంది: { $error }

journal.cli.describe.pipeline.retry.io.parent.identity = ప్రచురణకు ముందు బైండ్ అయిన పేరెంట్ గుర్తింపు మారింది

journal.cli.describe.pipeline.retry.io.parent.kind = పేరెంట్ నిజమైన డైరెక్టరీ కాదు

journal.cli.describe.pipeline.retry.io.parent.rename = ప్రచురణకు ముందు పేరెంట్ పాత్ పేరు మారింది

journal.cli.describe.pipeline.retry.io.parent.reparse = పేరెంట్ రిపార్స్ పాయింట్ అయి ఉంది

journal.cli.describe.pipeline.retry.io.prepare = సిద్ధం చేస్తుండగా స్టేజ్ గుర్తింపు మారింది

journal.cli.describe.pipeline.retry.io.stage.identity = ప్రచురణకు ముందు స్టేజ్ గుర్తింపు మారింది

journal.cli.describe.pipeline.retry.io.stage.namespace = ప్రచురణకు ముందు స్టేజ్ నేమ్‌స్పేస్ మారింది

journal.cli.describe.pipeline.retry.io.tmp = ప్రత్యేక తాత్కాలిక ఫైల్ కేటాయించలేకపోయింది

journal.cli.describe.pipeline.retry.journal = స్థానిక మళ్ళీ ప్రయత్నం జర్నల్ సరిపోలేదు

journal.cli.describe.pipeline.retry.open = { $ocr } మళ్ళీ ప్రయత్నం కోసం వీడియోను మళ్ళీ తెరవలేకపోయింది

journal.cli.describe.pipeline.retry.permission.changed = స్థానిక మళ్ళీ ప్రయత్నం అనుమతి మారింది

journal.cli.describe.pipeline.retry.permission.missing = స్థానిక మళ్ళీ ప్రయత్నం అనుమతి లేదు

journal.cli.describe.pipeline.root.canonicalize.journal.root = జర్నల్ రూట్ క్యానానికల్ చేయడం: { $detail }

journal.cli.describe.pipeline.root.cloud.files.sync.root.status.could.not.be.verified = { $cloud_files } సింక్-రూట్ స్థితి ధృవీకరించలేకపోయింది

journal.cli.describe.pipeline.root.filesystem.is.not.ntfs.or.refs = ఫైల్‌సిస్టమ్ { $ntfs } లేదా { $refs } కాదు

journal.cli.describe.pipeline.root.filesystem.type.could.not.be.verified = ఫైల్‌సిస్టమ్ రకం ధృవీకరించలేకపోయింది

journal.cli.describe.pipeline.root.journal.root.contains.a.reparse.point = జర్నల్ రూట్‌లో రిపార్స్ పాయింట్ ఉంది

journal.cli.describe.pipeline.root.journal.root.contains.an.interior.nul = జర్నల్ రూట్‌లో లోపల { $nul } ఉంది

journal.cli.describe.pipeline.root.journal.root.has.an.invalid.portable.name = జర్నల్ రూట్‌కు చెల్లని పోర్టబుల్ పేరు ఉంది

journal.cli.describe.pipeline.root.journal.root.is.a.cloud.files.sync.root = జర్నల్ రూట్ { $cloud_files } సింక్ రూట్ అయి ఉంది

journal.cli.describe.pipeline.root.journal.root.is.a.reparse.point = జర్నల్ రూట్ రిపార్స్ పాయింట్ అయి ఉంది

journal.cli.describe.pipeline.root.journal.root.is.on.a.non.local.volume = జర్నల్ రూట్ లోకల్ కాని వాల్యూమ్‌పై ఉంది

journal.cli.describe.pipeline.root.journal.root.is.on.a.ram.disk = జర్నల్ రూట్ { $ram } డిస్క్‌పై ఉంది

journal.cli.describe.pipeline.root.journal.root.is.on.removable.or.optical.media = జర్నల్ రూట్ తీసివేయగల లేదా ఆప్టికల్ మీడియాపై ఉంది

journal.cli.describe.pipeline.root.journal.root.must.be.lexically.canonical = జర్నల్ రూట్ లెక్సికల్‌గా క్యానానికల్ అయి ఉండాలి

journal.cli.describe.pipeline.root.journal.root.must.use.a.fully.qualified.drive.path = జర్నల్ రూట్ పూర్తి అర్హత గల డ్రైవ్ పాత్ ఉపయోగించాలి

journal.cli.describe.pipeline.root.journal.root.volume.type.is.unknown = జర్నల్ రూట్ వాల్యూమ్ రకం తెలియదు

journal.cli.describe.pipeline.root.open.filesystem.root = ఫైల్‌సిస్టమ్ రూట్ తెరవడం: { $detail }

journal.cli.describe.pipeline.root.open.journal.directory = జర్నల్ డైరెక్టరీ తెరవడం: { $detail }

journal.cli.describe.pipeline.root.open.journal.root = జర్నల్ రూట్ తెరవడం: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.ancestor = జర్నల్ రూట్ పూర్వీకం తెరవడం: { $detail }

journal.cli.describe.pipeline.root.open.journal.root.namespace.watch = జర్నల్ రూట్ నేమ్‌స్పేస్ వాచ్ తెరవడం: { $detail }

journal.cli.describe.pipeline.root.open.verified.drive.root = ధృవీకరించిన డ్రైవ్ రూట్ తెరవడం: { $detail }

journal.cli.describe.pipeline.root.open.verified.journal.root = ధృవీకరించిన జర్నల్ రూట్ తెరవడం: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.ancestor.attributes = జర్నల్ రూట్ పూర్వీకం ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.attributes = జర్నల్ రూట్ ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.drive.type = జర్నల్ రూట్ డ్రైవ్ రకం క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.identity = జర్నల్ రూట్ గుర్తింపు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.attributes = జర్నల్ రూట్ నేమ్‌స్పేస్-వాచ్ ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.journal.root.namespace.watch.identity = జర్నల్ రూట్ నేమ్‌స్పేస్-వాచ్ గుర్తింపు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.attributes = నిలుపుకున్న జర్నల్ రూట్ ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.retained.journal.root.identity = నిలుపుకున్న జర్నల్ రూట్ గుర్తింపు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.attributes = ధృవీకరించిన డ్రైవ్ రూట్ ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.verified.drive.root.identity = ధృవీకరించిన డ్రైవ్ రూట్ గుర్తింపు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.attributes = ధృవీకరించిన జర్నల్ రూట్ ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.query.verified.journal.root.identity = ధృవీకరించిన జర్నల్ రూట్ గుర్తింపు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.root.read.source.file.identity = మూల ఫైల్ గుర్తింపు చదవడం: { $detail }

journal.cli.describe.pipeline.root.restat.acquired.journal.root = restat పొందిన జర్నల్ రూట్: { $detail }

journal.cli.describe.pipeline.root.stat.acquired.journal.root = stat పొందిన జర్నల్ రూట్: { $detail }

journal.cli.describe.pipeline.root.stat.canonical.root.component = stat క్యానానికల్ రూట్ కాంపోనెంట్: { $detail }

journal.cli.describe.pipeline.root.stat.opened.canonical.root.component = stat తెరిచిన క్యానానికల్ రూట్ కాంపోనెంట్: { $detail }

journal.cli.describe.pipeline.root.stat.opened.filesystem.root = stat తెరిచిన ఫైల్‌సిస్టమ్ రూట్: { $detail }

journal.cli.describe.pipeline.row.frame = describe వరుస { $frame_key } వదిలేసింది

journal.cli.describe.pipeline.source.bytes = { $ocr } మూల బైట్‌లు మారాయి

journal.cli.describe.pipeline.source.changed = { $ocr } మూలం ప్రచురణకు ముందు మారింది

journal.cli.describe.pipeline.source.identity = { $ocr } మూల గుర్తింపు మారింది

journal.cli.describe.pipeline.source.open = { $ocr } మూలం తెరుస్తుండగా మారింది

journal.cli.describe.pipeline.source.plain = { $ocr } మూలం ప్లెయిన్ ఫైల్ కాదు

journal.cli.describe.pipeline.temp = describe తాత్కాలిక ఫైల్ కేటాయించలేకపోయింది

journal.cli.describe.pipeline.timestamp = describe ఫ్రేమ్ { $frame } టైమ్‌స్టాంప్‌ను వదిలేసింది

journal.cli.describe.pipeline.windows.cleanup = { $detail }; స్టేజ్ { $stage } తీసివేయలేకపోయింది: { $error }

journal.cli.describe.pipeline.windows.name.alternate.data.stream = పేరులో కోలన్ ఉంది

journal.cli.describe.pipeline.windows.name.control = పేరులో కంట్రోల్ అక్షరం ఉంది

journal.cli.describe.pipeline.windows.name.dot.component = '.' మరియు '..' చెల్లుబాటు అయ్యే పేర్లు కావు

journal.cli.describe.pipeline.windows.name.empty = పేరు ఖాళీగా ఉంది

journal.cli.describe.pipeline.windows.name.forbidden.character = పేరులో { $windows } అనుమతించని అక్షరం ఉంది

journal.cli.describe.pipeline.windows.name.reserved.device = { $windows } పేరును రిజర్వ్ చేసింది

journal.cli.describe.pipeline.windows.name.root.or.prefix = పేరు ఒకే సాపేక్ష కాంపోనెంట్ అయి ఉండాలి

journal.cli.describe.pipeline.windows.name.separator = పేరులో పాత్ సెపరేటర్ ఉంది

journal.cli.describe.pipeline.windows.name.too.long = పేరు { $length_limit } { $encoding } బైట్‌ల కంటే పొడవుగా ఉంది

journal.cli.describe.pipeline.windows.name.trailing.dot.or.space = పేరు చుక్క లేదా స్పేస్‌తో అంతమవుతుంది

journal.cli.describe.pipeline.windows.path.already.exists = ప్రచురణ పాత్ పూర్వీకం '{ $component }' క్రియేట్-ఓన్లీలో కనిపించింది

journal.cli.describe.pipeline.windows.path.classify.install.source = ఇన్‌స్టాల్ మూలం క్లాసిఫై చేయడం: { $detail }

journal.cli.describe.pipeline.windows.path.component = ప్రచురణ పాత్ కాంపోనెంట్ పోర్టబుల్ కాదు: { $detail }

journal.cli.describe.pipeline.windows.path.create.publication.path.ancestor = ప్రచురణ పాత్ పూర్వీకం సృష్టించడం: { $detail }

journal.cli.describe.pipeline.windows.path.dot.component = ప్రచురణ పాత్‌లో '.' లేదా '..' ఉంది

journal.cli.describe.pipeline.windows.path.drive.relative = ప్రచురణ పాత్ డ్రైవ్-సాపేక్షం

journal.cli.describe.pipeline.windows.path.empty = ప్రచురణ పాత్ ఖాళీగా ఉంది

journal.cli.describe.pipeline.windows.path.identity.changed = నిలుపుకున్న స్థాయి { $level } వద్ద ప్రచురణ పాత్ గుర్తింపు మారింది

journal.cli.describe.pipeline.windows.path.identity.mismatch = ప్రచురణ పాత్ పూర్వీకం '{ $component }' గుర్తింపు సృష్టించడం మరియు మళ్ళీ తెరవడం మధ్య మారింది

journal.cli.describe.pipeline.windows.path.interior.nul = ప్రచురణ పాత్‌లో లోపల { $nul } ఉంది

journal.cli.describe.pipeline.windows.path.missing = ప్రచురణ పాత్ కాంపోనెంట్ '{ $component }' లేదు

journal.cli.describe.pipeline.windows.path.move.spelling.diverged = ప్రచురణ పాత్ తరలింపు స్పెల్లింగ్ ఇక టర్మినల్ పేరెంట్‌కు పేరు పెట్టదు

journal.cli.describe.pipeline.windows.path.not.directory = ప్రచురణ పాత్ పూర్వీకం '{ $component }' డైరెక్టరీ కాదు

journal.cli.describe.pipeline.windows.path.not.regular.file = ప్రచురణ పాత్ లీఫ్ '{ $component }' సాధారణ ఫైల్ కాదు

journal.cli.describe.pipeline.windows.path.open.install.source = ఇన్‌స్టాల్ మూలం తెరవడం: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.ancestor = ప్రచురణ పాత్ పూర్వీకం తెరవడం: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.anchor = ప్రచురణ పాత్ యాంకర్ తెరవడం: { $detail }

journal.cli.describe.pipeline.windows.path.open.publication.path.move.spelling = ప్రచురణ పాత్ తరలింపు స్పెల్లింగ్ తెరవడం: { $detail }

journal.cli.describe.pipeline.windows.path.path.too.long = ప్రచురణ పాత్ ఎక్స్‌టెండెడ్-లెంగ్త్ బౌండ్‌ను దాటింది

journal.cli.describe.pipeline.windows.path.query.install.path.attributes = ఇన్‌స్టాల్ పాత్ ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.windows.path.query.install.source.identity = ఇన్‌స్టాల్ మూలం గుర్తింపు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.attributes = ప్రచురణ పాత్ ఆట్రిబ్యూట్‌లు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.windows.path.query.publication.path.identity = ప్రచురణ పాత్ గుర్తింపు క్వెరీ చేయడం: { $detail }

journal.cli.describe.pipeline.windows.path.reopen.publication.path.ancestor = ప్రచురణ పాత్ పూర్వీకం మళ్ళీ తెరవడం: { $detail }

journal.cli.describe.pipeline.windows.path.reparse.point = ప్రచురణ పాత్ పూర్వీకం '{ $component }' రిపార్స్ పాయింట్ అయి ఉంది

journal.cli.describe.pipeline.windows.path.resolve.publication.path.current.directory = ప్రచురణ పాత్ ప్రస్తుత డైరెక్టరీ రిజల్వ్ చేయడం: { $detail }

journal.cli.describe.pipeline.windows.path.revalidate.install.source.after.classification = క్లాసిఫికేషన్ తర్వాత ఇన్‌స్టాల్ మూలం మళ్ళీ సరిచూడటం: { $detail }

journal.cli.describe.pipeline.windows.path.root.only = ప్రచురణ పాత్ గమ్యం లేని డ్రైవ్ రూట్

journal.cli.describe.pipeline.windows.path.rooted.without.drive = ప్రచురణ పాత్ డ్రైవ్ లేకుండా రూట్ అయి ఉంది

journal.cli.describe.pipeline.windows.path.unc.or.device.prefix = ప్రచురణ పాత్‌కు { $unc } లేదా పరికరం ప్రిఫిక్స్ ఉంది

journal.cli.describe.pipeline.windows.path.unstable = ప్రచురణ పాత్‌కు { $manager } వాల్యూమ్ { $guid } లేదు: { $detail }

journal.cli.describe.pipeline.withdraw = స్క్రీన్ { $ocr } ఉపసంహరణ సిద్ధం చేయలేకపోయింది: { $detail }

journal.cli.describe.settings.finite = { $setting } ఫైనైట్ సంఖ్య అయి ఉండాలి

journal.cli.describe.settings.importance = { $setting } { $ignore }, { $low }, { $normal }, లేదా { $high } అయి ఉండాలి

journal.cli.describe.settings.object = { $setting } ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.describe.settings.string = { $setting } స్ట్రింగ్ అయి ఉండాలి

journal.cli.describe.settings.strings = { $setting } స్ట్రింగ్‌ల అర్రే అయి ఉండాలి

journal.cli.describe.settings.uint = { $setting } అన్‌సైన్డ్ 32-బిట్ పూర్ణాంకం అయి ఉండాలి

journal.cli.describe.stored.error.empty = { $category } కోసం ఖాళీ మార్క్‌డౌన్ రెస్పాన్స్

journal.cli.describe.stored.error.incomplete = వెలికితీత ఎప్పుడూ పూర్తి కాలేదు

journal.cli.describe.stored.error.invalid = చెల్లని { $format } రెస్పాన్స్

journal.cli.describe.stored.error.invalid.category = చెల్లని { $format } రెస్పాన్స్ { $category } కోసం: { $error }

journal.cli.describe.stored.error.resize = వెలికితీత ఫ్రేమ్ పరిమాణం మార్చలేకపోయింది

journal.cli.describe.stored.error.schema = { $category } కోసం స్కీమా సరిచూపు విఫలమైంది

journal.cli.describe.stored.error.schema.detail = { $category } కోసం స్కీమా సరిచూపు విఫలమైంది: { $errors }

journal.cli.describe.stored.error.truncated = { $category } కోసం కత్తిరించిన మార్క్‌డౌన్ రెస్పాన్స్

journal.cli.describe.stored.note.bounded = [{ $mark }: వెలికితీత అవుట్‌పుట్ జర్నల్‌లో రాయడానికి ముందు పరిమితం అయింది - డీజెనరేట్ పొడవు శానిటైజ్ చేయబడింది/కత్తిరించబడింది]

journal.cli.describe.stored.note.ocr = మీ స్క్రీన్‌ల నుండి గుర్తించిన వచనం. దీనిలో గుర్తింపు లోపాలు ఉండవచ్చు. ఇది కనిపించినది చెబుతుంది, మీరు చదివినది, టైప్ చేసినది లేదా చేసినది కాదు. పాస్‌వర్డ్‌లను మరియు ఇతర రహస్యాలను మాస్క్ చేయడం వీలైనంతవరకు; మీ రిడాక్షన్ నియమాలు వర్తించవు.

journal.cli.describe.stored.ocr.heading = { $clock } · { $category }

journal.cli.describe.usage.argument.unknown = usage: { $syntax }{ $line_break }{ $command }: లోపం: తెలియని ఆర్గ్యుమెంట్: { $argument }

journal.cli.describe.usage.backfill.arguments = usage: { $syntax }{ $line_break }{ $command }: లోపం: ముఖ బ్యాక్‌ఫిల్‌కు { $arguments } కావాలి

journal.cli.describe.usage.backfill.budget = usage: { $syntax }{ $line_break }{ $command }: లోపం: ముఖ బ్యాక్‌ఫిల్ బడ్జెట్ అనుమతించిన పరిధి బయట ఉంది

journal.cli.describe.usage.backfill.desktop = usage: { $syntax }{ $line_break }{ $command }: లోపం: ముఖ బ్యాక్‌ఫిల్ డెస్క్‌టాప్ జర్నల్‌లలో మాత్రమే అందుబాటులో ఉంది

journal.cli.describe.usage.describe.repeated = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $describe_flag } ఒకటి కంటే ఎక్కువసార్లు ఇవ్వబడింది

journal.cli.describe.usage.file.extra = usage: { $syntax }{ $line_break }{ $command }: లోపం: ఒక వీడియో పాత్ మాత్రమే ఇవ్వవచ్చు

journal.cli.describe.usage.file.required = usage: { $syntax }{ $line_break }{ $command }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $file }

journal.cli.describe.usage.frames.repeated = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $frames_only_flag } ఒకటి కంటే ఎక్కువసార్లు ఇవ్వబడింది

journal.cli.describe.usage.help = usage: { $syntax }

journal.cli.describe.usage.help.extra = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $help_flag } ఇతర ఆర్గ్యుమెంట్‌లు తీసుకోదు

journal.cli.describe.usage.jobs.positive = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $jobs_flag } ధనాత్మక పూర్ణాంకం కావాలి

journal.cli.describe.usage.jobs.repeated = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $jobs_flag } ఒకటి కంటే ఎక్కువసార్లు ఇవ్వబడింది

journal.cli.describe.usage.journal.path = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $journal_flag } పాత్ కావాలి

journal.cli.describe.usage.journal.repeated = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $journal_flag } ఒకటి కంటే ఎక్కువసార్లు ఇవ్వబడింది

journal.cli.describe.usage.mode.required = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $frames_only_flag } లేదా { $describe_flag } లో ఒక్కటే తప్పనిసరి

journal.cli.describe.usage.redo.mode = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $redo_flag } కు { $describe_flag } కావాలి

journal.cli.describe.usage.redo.repeated = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $redo_flag } ఒకటి కంటే ఎక్కువసార్లు ఇవ్వబడింది

journal.cli.describe.usage.registry.extra = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $category_registry_flag } ఇతర ఆర్గ్యుమెంట్‌లు తీసుకోదు

journal.cli.describe.usage.version.extra = usage: { $syntax }{ $line_break }{ $command }: లోపం: { $version_flag } ఇతర ఆర్గ్యుమెంట్‌లు తీసుకోదు

journal.cli.grab.alternatives.days = అందుబాటులో ఉన్న రోజులు (దగ్గరి 5):

journal.cli.grab.alternatives.screens = { $day }/{ $stream }/{ $segment } లో అందుబాటులో ఉన్న స్క్రీన్‌లు:

journal.cli.grab.alternatives.segments = { $day }/{ $stream } లో అందుబాటులో ఉన్న సెగ్మెంట్‌లు:

journal.cli.grab.alternatives.streams = { $day } లో అందుబాటులో ఉన్న ప్రవాహాలు:

journal.cli.grab.error.captured.not.analyzed = స్క్రీన్ { $screen } { $segment } లో క్యాప్చర్ అయింది కానీ విశ్లేషించబడలేదు

journal.cli.grab.error.day.not.found = రోజు { $day } దొరకలేదు

journal.cli.grab.error.decode.failed.ids = ఫ్రేమ్ ids డీకోడ్ చేయలేకపోయింది: { $ids }

journal.cli.grab.error.decode.invalid.rgb = వీడియో డీకోడ్ చేయలేకపోయింది: చెల్లని RGB ఫ్రేమ్

journal.cli.grab.error.decode.no.video = వీడియో డీకోడ్ చేయలేకపోయింది: వీడియో ప్రసారం లేదు

journal.cli.grab.error.decode.video = వీడియో డీకోడ్ చేయలేకపోయింది: { $error }

journal.cli.grab.error.decoder.init = వీడియో డీకోడర్ ఇనీషియలైజ్ చేయలేకపోయింది: { $error }

journal.cli.grab.error.force.requires.out = { $force_flag } కు { $out_flag } కావాలి

journal.cli.grab.error.frame.id.positive = ఫ్రేమ్ ids ధనాత్మక పూర్ణాంకాలు అయి ఉండాలి: వచ్చింది '{ $token }'

journal.cli.grab.error.frame.id.unique = ఫ్రేమ్ ids ప్రత్యేకం అయి ఉండాలి: { $frame_id }

journal.cli.grab.error.frame.not.found = ఫ్రేమ్ id { $frame_id } { $screen } లో { $segment } కోసం దొరకలేదు

journal.cli.grab.error.invalid.frame.id = చెల్లని ఫ్రేమ్ id

journal.cli.grab.error.invalid.level4 = చెల్లని స్థాయి 4 పేలోడ్

journal.cli.grab.error.invalid.level5a = చెల్లని స్థాయి 5a పేలోడ్

journal.cli.grab.error.invalid.render.payload = చెల్లని రెండర్ పేలోడ్

journal.cli.grab.error.invalid.segment.key = సెగ్మెంట్ { $segment } చెల్లుబాటు అయ్యే { $format } కీ కాదు

journal.cli.grab.error.io = { $path } యాక్సెస్ చేయలేకపోయింది: { $error }

journal.cli.grab.error.legacy.schema = స్క్రీన్ ఫైల్ frame_id కు ముందు స్కీమా వాడుతుంది; ఫ్రేమ్ ఎంపిక అందుబాటులో లేదు

journal.cli.grab.error.multiple.ids.require.out = అనేక ఫ్రేమ్ ids కు { $out_flag } కావాలి

journal.cli.grab.error.out.exists = అవుట్‌పుట్ పాత్ ఉంది ({ $force_flag } ఉపయోగించండి): { $paths }

journal.cli.grab.error.out.extension = { $out_flag } { $extensions } తో అంతమవ్వాలి

journal.cli.grab.error.out.filename = అవుట్‌పుట్ పాత్‌కు చెల్లుబాటు అయ్యే ఫైల్ పేరు లేదు

journal.cli.grab.error.out.requires.five.tokens = { $out_flag } కు day stream segment screen మరియు frame-id కావాలి

journal.cli.grab.error.save.image = చిత్రాన్ని సేవ్ చేయలేకపోయింది: { $error }

journal.cli.grab.error.screen.not.found = స్క్రీన్ { $screen } { $day }/{ $stream }/{ $segment } లో దొరకలేదు

journal.cli.grab.error.segment.not.found = సెగ్మెంట్ { $segment } { $day }/{ $stream } లో దొరకలేదు

journal.cli.grab.error.stream.not.found = ప్రవాహం { $stream } { $day } లో దొరకలేదు

journal.cli.grab.error.timestamp.finite = టైమ్‌స్టాంప్ ఫైనైట్ అయి ఉండాలి

journal.cli.grab.error.timestamp.invalid = చెల్లని టైమ్‌స్టాంప్

journal.cli.grab.error.timestamp.range = టైమ్‌స్టాంప్ పరిధి బయట ఉంది

journal.cli.grab.error.too.many.tokens = grab గరిష్ఠంగా 5 పొజిషనల్ టోకెన్‌లను తీసుకుంటుంది: day stream segment screen frame-id

journal.cli.grab.error.unsupported.level = మద్దతు లేని అవుట్‌పుట్ స్థాయి

journal.cli.grab.error.unsupported.level.named = మద్దతు లేని అవుట్‌పుట్ స్థాయి: { $level }

journal.cli.grab.error.video.missing = స్క్రీన్ { $screen } కోసం { $segment } లో ముడి వీడియో దొరకలేదు

journal.cli.grab.error.video.missing.with.reason = { $reason }; మెటాడేటా-మాత్రమే యాక్సెస్ ద్వారా మిగిలి ఉంది: { $command }

journal.cli.grab.frame.note.error = లోపం: { $text }

journal.cli.grab.header.abs.time = abs_time

journal.cli.grab.header.connector = కనెక్టర్

journal.cli.grab.header.day = రోజు

journal.cli.grab.header.end = ముగింపు

journal.cli.grab.header.frame.id = frame_id

journal.cli.grab.header.frames.analyzed = frames_analyzed

journal.cli.grab.header.notes = నోట్‌లు

journal.cli.grab.header.position = స్థానం

journal.cli.grab.header.primary = ప్రధాన

journal.cli.grab.header.screen = స్క్రీన్

journal.cli.grab.header.screens = స్క్రీన్‌లు

journal.cli.grab.header.segment = సెగ్మెంట్

journal.cli.grab.header.segments = సెగ్మెంట్‌లు

journal.cli.grab.header.start = ప్రారంభం

journal.cli.grab.header.status = స్థితి

journal.cli.grab.header.stream = ప్రవాహం

journal.cli.grab.header.streams = ప్రవాహాలు

journal.cli.grab.header.timestamp = టైమ్‌స్టాంప్

journal.cli.grab.level4.empty = ఈ స్క్రీన్ విశ్లేషణలో క్వాలిఫై అయిన ఫ్రేమ్‌లు లేవు.

journal.cli.grab.level4.footer = పరిశీలన: { $inspect_command }{ $line_break }ఒకటి సేవ్: { $save_one_command }{ $line_break }చాలా సేవ్: { $save_many_command }{ $line_break }{ $line_break }వెలికితీత ఎలా పని చేస్తుంది:{ $line_break }  డీకోడింగ్ వీడియోను ఫ్రేమ్ 0 నుండి సరళంగా నడుపుతుంది — సీక్ చేయడం సురక్షితం కాదు, ఈ{ $line_break }  1 Hz క్యాప్చర్ రేటు వద్ద. ఖర్చు అత్యధికంగా అడిగిన frame_id మీద ఆధారపడుతుంది, కాదు{ $line_break }  సంఖ్య మీద. 7,12,23 ఐడిలు అడగడం ఒక్క 23 ను అడిగినంతే ఖర్చు.{ $line_break }  ఒకే స్క్రీన్ నుండి ఒకటి కంటే ఎక్కువ ఫ్రేమ్‌లు మీకు కావాలంటే బ్యాచ్ మోడ్ మేలు.

journal.cli.grab.level4.legacy = 0 ఫ్రేమ్‌లు విశ్లేషించబడ్డాయి: ఫైల్ frame_id కు ముందు ఉన్న స్కీమాను వాడుతుంది

journal.cli.grab.level4.missing.video = సేవ్ మోడ్ అందుబాటులో లేదు: { $reason }.{ $line_break }పైన ఉన్న ఫ్రేమ్ మెటాడేటా ఇంకా చదవగలిగేది.{ $line_break }{ $line_break }పరిశీలన: { $command }

journal.cli.grab.level5a.footer = సేవ్: { $save_command }{ $line_break }బ్యాచ్: { $batch_command }

journal.cli.grab.level5a.frame = ఫ్రేమ్: { $value }

journal.cli.grab.level5a.jsonl = JSONL: { $value }

journal.cli.grab.level5a.notes = నోట్‌లు: { $value }

journal.cli.grab.level5a.screen = స్క్రీన్: { $value }

journal.cli.grab.level5a.time = సమయం: { $value }

journal.cli.grab.level5a.video = వీడియో: { $value }

journal.cli.grab.missing.reason.offload = మీ బ్యాకప్ దాన్ని కాపీ చేసిన తర్వాత మీరు అసలు వీడియోను తొలగించారు

journal.cli.grab.missing.reason.owner = మీరు అసలు వీడియోను తొలగించారు

journal.cli.grab.missing.reason.policy = మీ నిలుపుదల సెట్టింగ్‌లు దాన్ని గుర్తు పెట్టిన తర్వాత మీరు అసలు వీడియోను తొలగించారు

journal.cli.grab.missing.reason.trash = అసలు వీడియో ట్రాష్‌లో ఉంది, అక్కడ మీరు సెట్టింగ్‌లలో దాన్ని రీస్టోర్ చేయగలరు

journal.cli.grab.missing.reason.unknown = అసలు వీడియో ఇక మీ జర్నల్‌లో లేదు

journal.cli.grab.next = తర్వాత: { $command }

journal.cli.grab.saved = { $path } సేవ్ అయింది

journal.cli.grab.status.analyzed = విశ్లేషించబడింది

journal.cli.grab.status.captured = క్యాప్చర్ అయింది కానీ విశ్లేషించబడలేదు

journal.cli.grab.status.missing.offload = విశ్లేషించబడింది; మీ బ్యాకప్ దాన్ని కాపీ చేసిన తర్వాత మీరు అసలు వీడియోను తొలగించారు

journal.cli.grab.status.missing.owner = విశ్లేషించబడింది; మీరు అసలు వీడియోను తొలగించారు

journal.cli.grab.status.missing.policy = విశ్లేషించబడింది; మీ నిలుపుదల సెట్టింగ్‌లు దాన్ని గుర్తు పెట్టిన తర్వాత మీరు అసలు వీడియోను తొలగించారు

journal.cli.grab.status.missing.trash = విశ్లేషించబడింది; అసలు వీడియో ట్రాష్‌లో ఉంది

journal.cli.grab.status.missing.unknown = విశ్లేషించబడింది; అసలు వీడియో ఇక మీ జర్నల్‌లో లేదు

journal.cli.maintenance.backup.erase = బ్యాకప్ తుడిచివేయడం: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending }

journal.cli.maintenance.backup.erase.error.detail = బ్యాకప్ తుడిచివేయడం: లోపం { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.erase.reason = బ్యాకప్ తుడిచివేయడం: { $state } { $removed_field }{ $removed } { $shared_field }{ $shared } { $staying_field }{ $staying } { $unreached_field }{ $unreached } { $pending_field }{ $pending } { $reason_field }{ $reason }

journal.cli.maintenance.backup.error = బ్యాకప్: లోపం { $reason_field }{ $reason }

journal.cli.maintenance.backup.error.detail = బ్యాకప్: లోపం { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.ok = బ్యాకప్: ok { $field }{ $snapshot }

journal.cli.maintenance.backup.paused = బ్యాకప్: దాటవేయబడింది. బ్యాకప్ పాజ్ అయి ఉంది, కాబట్టి ఏమీ నడవలేదు. మళ్ళీ స్టార్ట్ చేయడానికి `{ $resume }` రన్ చేయండి.

journal.cli.maintenance.backup.prune.error = బ్యాకప్ ప్రూన్: లోపం { $reason_field }{ $reason }

journal.cli.maintenance.backup.prune.error.detail = బ్యాకప్ ప్రూన్: లోపం { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.prune.ok = బ్యాకప్ ప్రూన్: ok

journal.cli.maintenance.backup.prune.paused = బ్యాకప్ ప్రూన్: దాటవేయబడింది. బ్యాకప్ పాజ్ అయి ఉంది, కాబట్టి ఏమీ నడవలేదు. మళ్ళీ స్టార్ట్ చేయడానికి `{ $resume }` రన్ చేయండి.

journal.cli.maintenance.backup.prune.skipped = బ్యాకప్ ప్రూన్: దాటవేయబడింది

journal.cli.maintenance.backup.skipped = బ్యాకప్: దాటవేయబడింది

journal.cli.maintenance.backup.superseded = బ్యాకప్: మరో పరికరం దీన్ని క్లెయిమ్ చేసింది; లోకల్ సెట్టింగ్‌లు ఖాళీ చేయబడ్డాయి. ఇక్కడ కొత్త బ్యాకప్ సెటప్ చేయడానికి `{ $enable }` రన్ చేయండి.

journal.cli.maintenance.backup.unavailable = బ్యాకప్ మెయింటెనెన్స్ సేవలు అందుబాటులో లేవు.

journal.cli.maintenance.backup.usage.offload = usage: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $args }

journal.cli.maintenance.backup.usage.routine = usage: { $command } { $id }{ $options }{ $line_break }{ $command } { $id }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $args }

journal.cli.maintenance.backup.verify.error = బ్యాకప్ సరిచూపు: లోపం { $reason_field }{ $reason }

journal.cli.maintenance.backup.verify.error.detail = బ్యాకప్ సరిచూపు: లోపం { $reason_field }{ $reason } { $detail_field }{ $detail }

journal.cli.maintenance.backup.verify.ok = బ్యాకప్ సరిచూపు: ok { $field }{ $subset }

journal.cli.maintenance.backup.verify.paused = బ్యాకప్ సరిచూపు: దాటవేయబడింది. బ్యాకప్ పాజ్ అయి ఉంది, కాబట్టి ఏమీ నడవలేదు. మళ్ళీ స్టార్ట్ చేయడానికి `{ $resume }` రన్ చేయండి.

journal.cli.maintenance.backup.verify.skipped = బ్యాకప్ సరిచూపు: దాటవేయబడింది

journal.cli.maintenance.description.backup.erase = మీరు ఎంచుకున్నప్పుడు, మీరు తుడిచివేసినదాన్ని మీ బ్యాకప్‌ల నుండి కూడా తీసివేయండి.

journal.cli.maintenance.description.backup.offload = బ్యాకప్ తర్వాత సరిచూసిన ముడి మీడియాను బయటికి తరలించండి.

journal.cli.maintenance.description.backup.prune = ఎన్‌క్రిప్ట్ చేసిన బ్యాకప్ నిలుపుదల విధానాన్ని అమలు చేయండి.

journal.cli.maintenance.description.backup.run = ఎన్‌క్రిప్ట్ చేసిన బ్యాకప్ రన్ చేయండి.

journal.cli.maintenance.description.backup.verify = ఎన్‌క్రిప్ట్ చేసిన బ్యాకప్ తిరిగి చదవడాన్ని సరిచూడండి.

journal.cli.maintenance.description.faces.backfill = సమావేశాల నుండి, గత సమావేశాలతో సహా, ప్రొఫైల్ చిత్రం సూచనలను రిఫ్రెష్ చేయండి.

journal.cli.maintenance.description.health.mark.raw = తీసివేయడానికి సిద్ధంగా ఉన్న అసలు మీడియాను జాబితా చేయండి.

journal.cli.maintenance.description.health.prune.logs = పాత ఆపరేషన్ లాగ్‌లను ప్రూన్ చేయండి.

journal.cli.maintenance.description.retention.owner.voice.originals = "ఇతర గొంతులను మినహాయించడం" ఆన్‌తో ప్రాసెస్ అయిన ఇటీవలి రికార్డింగ్‌ల అసలు ఆడియో తొలగింపును పూర్తి చేయండి.

journal.cli.maintenance.description.speakers.backfill.labels = ఇంకా మాట్లాడిన వారి లేబుల్‌లు లేని ఆడియోలో, మోడల్ లేకుండా, మాట్లాడిన వారికి పేరు పెట్టండి.

journal.cli.maintenance.description.speakers.backfill.pool = ఇప్పటికే ట్రాన్స్క్రిప్ట్ అయిన ఆడియో నుండి గొంతుల జాబితాను తాజా చేసి, తర్వాత మీ గొంతును వెతకండి.

journal.cli.maintenance.description.speakers.candidate.pair.suggestions = సూచనల కోసం దట్టమైన మాట్లాడిన వారి అభ్యర్థి జతలను కనుగొనండి.

journal.cli.maintenance.description.speakers.consolidate.pool = దట్టమైన మాట్లాడిన వారి అభ్యర్థులను కన్సాలిడేట్ చేయండి.

journal.cli.maintenance.description.speakers.discover.voices = తిరిగి వచ్చే గొంతు కనుగొనే కాష్‌ను రిఫ్రెష్ చేయండి.

journal.cli.maintenance.description.speakers.name.variants = సూచనల కోసం మాట్లాడిన వారి పేరు వేరియంట్‌లను కనుగొనండి.

journal.cli.maintenance.description.trash.erase = ఆపడం అంతరాయం కలిగించిన ట్రాష్ దశలను పూర్తి చేసి, రోజులు వేచి ఉన్నదాన్ని తుడిచివేయండి.

journal.cli.maintenance.face.generation = ముఖ బ్యాక్‌ఫిల్ జనరేషన్ అంగీకరించబడలేదు

journal.cli.maintenance.face.log.cache = ముఖ బ్యాక్‌ఫిల్ స్కాన్ కాష్ సేవ్ కాలేదు: { $detail }

journal.cli.maintenance.face.log.deferred = ముఖ పాల్గొనడం రికవరీ వాయిదాలో ఉంది: { $detail }

journal.cli.maintenance.face.log.failed = ముఖ బ్యాక్‌ఫిల్ విఫలమైంది: { $detail }

journal.cli.maintenance.face.priority = ముఖ చరిత్ర ప్రాధాన్యత అందుబాటులో లేదు: { $error }

journal.cli.maintenance.face.reconciliation = ముఖ సమన్వయం: { $error }

journal.cli.maintenance.health.config.days = { $field } ధనాత్మక పూర్ణాంకం అయి ఉండాలి

journal.cli.maintenance.health.config.enabled = { $field } బూలియన్ అయి ఉండాలి

journal.cli.maintenance.health.keep = { $routine }: మీ నిలుపుదల సెట్టింగ్‌లు అసలు మీడియా అంతటినీ ఉంచుతాయి.

journal.cli.maintenance.health.mark.entry = { $indent }{ $id }: { $reason }

journal.cli.maintenance.health.mark.refused = { $routine }: కొన్ని అంశాలను జాబితా చేయలేకపోయింది:

journal.cli.maintenance.health.mark.summary = { $routine }: కొత్త అంశాలు: { $new }.{ $line_break }  నిలిచి ఉన్న మొత్తం: { $total }.

journal.cli.maintenance.health.mark.unavailable = { $routine }: జాబితాను బిల్డ్ చేయలేకపోయింది: { $error }

journal.cli.maintenance.health.prune.args.integer = usage: { $command } { $syntax }{ $line_break }{ $command }: లోపం: ఆర్గ్యుమెంట్ { $flag }: చెల్లని పూర్ణాంకం విలువ: '{ $value }'

journal.cli.maintenance.health.prune.args.missing = usage: { $command } { $syntax }{ $line_break }{ $command }: లోపం: ఆర్గ్యుమెంట్ { $flag }: ఒక ఆర్గ్యుమెంట్ అవసరం

journal.cli.maintenance.health.prune.args.positive = { $routine }: { $flag } ధనాత్మక పూర్ణాంకం అయి ఉండాలి

journal.cli.maintenance.health.prune.args.unexpected = usage: { $command } { $syntax }{ $line_break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $value }

journal.cli.maintenance.health.prune.class = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } ఫైల్, { $dirs } కాష్ డైరెక్టరీ, { $bytes }
       *[other] { $indent }{ $class }: { $files } ఫైల్, { $dirs } కాష్ డైరెక్టరీలు, { $bytes }
    }
   *[other] { $dirs_quantity ->
        [one] { $indent }{ $class }: { $files } ఫైల్‌లు, { $dirs } కాష్ డైరెక్టరీ, { $bytes }
       *[other] { $indent }{ $class }: { $files } ఫైల్‌లు, { $dirs } కాష్ డైరెక్టరీలు, { $bytes }
    }
}

journal.cli.maintenance.health.prune.disabled = { $routine }: డిసేబుల్

journal.cli.maintenance.health.prune.halted = { $routine }: { $message }

journal.cli.maintenance.health.prune.missing = జర్నల్ డైరెక్టరీ అందుబాటులో లేదు

journal.cli.maintenance.health.prune.preview = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్, { $dirs } కాష్ డైరెక్టరీ, { $bytes } ప్రూన్ చేసేది { $cutoff_field }{ $cutoff }
       *[other] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్, { $dirs } కాష్ డైరెక్టరీలు, { $bytes } ప్రూన్ చేసేది { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్‌లు, { $dirs } కాష్ డైరెక్టరీ, { $bytes } ప్రూన్ చేసేది { $cutoff_field }{ $cutoff }
       *[other] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్‌లు, { $dirs } కాష్ డైరెక్టరీలు, { $bytes } ప్రూన్ చేసేది { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.queue = ఒక లాగ్ తీసివేయబడింది, కానీ దాని సెర్చ్ అప్‌డేట్‌ను వరుసలో పెట్టలేకపోయింది: { $error }

journal.cli.maintenance.health.prune.refused = { $routine }: కొన్ని లాగ్‌లను ప్రూన్ చేయలేకపోయింది: { $summary }

journal.cli.maintenance.health.prune.refused.unnamed = { $routine }: కొన్ని లాగ్‌లను ప్రూన్ చేయలేకపోయింది: నిలుపుదల టూల్ ఒక ఎంట్రీ పేరు చెప్పకుండా నిరాకరించింది.

journal.cli.maintenance.health.prune.result = { $files_quantity ->
    [one] { $dirs_quantity ->
        [one] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్, { $dirs } కాష్ డైరెక్టరీ, { $bytes } ప్రూన్ అయింది { $cutoff_field }{ $cutoff }
       *[other] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్, { $dirs } కాష్ డైరెక్టరీలు, { $bytes } ప్రూన్ అయింది { $cutoff_field }{ $cutoff }
    }
   *[other] { $dirs_quantity ->
        [one] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్‌లు, { $dirs } కాష్ డైరెక్టరీ, { $bytes } ప్రూన్ అయింది { $cutoff_field }{ $cutoff }
       *[other] { $routine }: { $files } ఆపరేషన్ లాగ్ ఫైల్‌లు, { $dirs } కాష్ డైరెక్టరీలు, { $bytes } ప్రూన్ అయింది { $cutoff_field }{ $cutoff }
    }
}

journal.cli.maintenance.health.prune.unavailable = { $routine }: లాగ్‌లను ప్రూన్ చేయలేకపోయింది: { $error }

journal.cli.maintenance.health.prune.undateable = { $indent }లోపం: ఎంట్రీ నిలుపుదల తేదీని నిర్ణయించలేకపోయింది: ఎంట్రీ నిలుపుదల తేదీని నిర్ణయించలేకపోయింది

journal.cli.maintenance.health.prune.unreadable = { $indent }లోపం: { $detail }: { $detail }

journal.cli.maintenance.health.unavailable = స్థితి మెయింటెనెన్స్ సేవలు అందుబాటులో లేవు.

journal.cli.maintenance.health.usage = usage: { $command } { $id } { $syntax }{ $line_break }{ $command } { $id }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $args }

journal.cli.maintenance.help.list = usage: { $syntax }

journal.cli.maintenance.help.root = usage: { $syntax }

journal.cli.maintenance.help.run = usage: { $syntax }

journal.cli.maintenance.help.sync = usage: { $syntax }

journal.cli.maintenance.list.empty = మెయింటెనెన్స్ రొటీన్‌లు ఏవీ దొరకలేదు.

journal.cli.maintenance.list.heading.description = వివరణ

journal.cli.maintenance.list.heading.every = EVERY

journal.cli.maintenance.list.heading.id = ID

journal.cli.maintenance.list.heading.runtime = గరిష్ఠ రన్‌టైమ్

journal.cli.maintenance.list.heading.status = STATUS

journal.cli.maintenance.list.status.divergent = విభిన్నం

journal.cli.maintenance.list.status.missing = దొరకలేదు

journal.cli.maintenance.list.status.nothere = వర్తించదు

journal.cli.maintenance.list.status.synced = సింక్ అయింది

journal.cli.maintenance.missing.root = usage: { $syntax }{ $line_break }{ $command }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $argument }

journal.cli.maintenance.missing.run = usage: { $syntax }{ $line_break }{ $command }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $argument }

journal.cli.maintenance.retention.import.owed = ఇంపోర్ట్ అయిన ఆడియో క్లీనప్ పూర్తి కాలేదు: { $error }

journal.cli.maintenance.retention.owed = మీ గొంతు అసలు క్లీనప్ పూర్తి కాలేదు: { $error }

journal.cli.maintenance.retention.summary = { $routine }: తొలగించినవి { $deleted }, నమోదు లేకుండా తొలగించినవి { $deleted_unrecorded }, ఇప్పటికే పోయినవి { $gone }, ఇంకా బాకీ ఉన్నవి { $owed }, శాశ్వతంగా ఉంచినవి { $refused }, నిరూపించలేనివి { $not_provable }, బాకీ లేనివి { $not_owed }, బిజీగా ఉన్నప్పుడు దాటవేసినవి { $skipped }, చదవలేని ఈవెంట్‌లు { $unknown }

journal.cli.maintenance.retention.usage = usage: { $id } ఆర్గ్యుమెంట్‌లు తీసుకోదు.

journal.cli.maintenance.routine.unimplemented = { $count_quantity ->
    [one] మెయింటెనెన్స్ రొటీన్ { $id } ఇంకా ఇంప్లిమెంట్ కాలేదు ({ $count } ఫార్వర్డ్ చేసిన ఆర్గ్యుమెంట్).
   *[other] మెయింటెనెన్స్ రొటీన్ { $id } ఇంకా ఇంప్లిమెంట్ కాలేదు ({ $count } ఫార్వర్డ్ చేసిన ఆర్గ్యుమెంట్‌లు).
}

journal.cli.maintenance.routine.unknown = తెలియని మెయింటెనెన్స్ రొటీన్: { $id }. అందుబాటులో ఉన్న రొటీన్‌లు చూడటానికి `{ $command }` రన్ చేయండి.

journal.cli.maintenance.schedule.error = { $path } చదవడంలో/అప్‌డేట్ చేయడంలో లోపం: { $error } (కారణం: { $error })

journal.cli.maintenance.schedule.io = షెడ్యూల్ I/O విఫలమైంది: { $detail }

journal.cli.maintenance.schedule.malformed = { $path } వద్ద షెడ్యూల్‌ల కాన్ఫిగ్ ఆకృతి తప్పు

journal.cli.maintenance.schedule.metadata = తెలియని షెడ్యూల్ మెటాడేటా కీలు: { $keys }

journal.cli.maintenance.schedule.shape = { $path }: షెడ్యూల్‌ల కాన్ఫిగ్ JSON ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.maintenance.schedule.state.shape = { $path } వద్ద షెడ్యూల్ స్థితి JSON ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.maintenance.speakers.discovery.helper = మాట్లాడిన వారిని కనుగొనే సహాయకం { $stage } వద్ద విఫలమైంది: { $reason }

journal.cli.maintenance.speakers.discovery.input = మాట్లాడిన వారిని కనుగొనడం విఫలమైంది: { $detail }

journal.cli.maintenance.speakers.error.owner.voice.look.failed = మీ గొంతు వెతకడం విఫలమైంది: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.consolidation.failed = మాట్లాడిన వారి అభ్యర్థి కన్సాలిడేట్ విఫలమైంది: { $error }

journal.cli.maintenance.speakers.error.speaker.candidate.pair.refresh.failed = మాట్లాడిన వారి అభ్యర్థి జత రిఫ్రెష్ విఫలమైంది: { $error }

journal.cli.maintenance.speakers.error.speaker.label.backfill.failed = మాట్లాడిన వారి లేబుల్ బ్యాక్‌ఫిల్ విఫలమైంది: { $error }

journal.cli.maintenance.speakers.error.speaker.name.variant.refresh.failed = మాట్లాడిన వారి పేరు వేరియంట్ రిఫ్రెష్ విఫలమైంది: { $error }

journal.cli.maintenance.speakers.not.admitted = మెయింటెనెన్స్ ఎంట్రీ వద్ద మాట్లాడిన వారిని కనుగొనే జనరేషన్ అంగీకరించబడలేదు

journal.cli.maintenance.speakers.usage.backfill.labels = usage: { $command }

journal.cli.maintenance.speakers.usage.backfill.pool = usage: { $command }

journal.cli.maintenance.speakers.usage.candidate.pair.suggestions = usage: { $command }

journal.cli.maintenance.speakers.usage.consolidate.pool = usage: { $command }

journal.cli.maintenance.speakers.usage.discover.voices = usage: { $command }

journal.cli.maintenance.speakers.usage.name.variants = usage: { $command }

journal.cli.maintenance.sync.added = చేర్చబడింది: { $count }: { $ids }

journal.cli.maintenance.sync.added.empty = చేర్చబడింది: 0

journal.cli.maintenance.sync.disabled = డిసేబుల్: { $count }: { $ids }

journal.cli.maintenance.sync.disabled.empty = డిసేబుల్: 0

journal.cli.maintenance.sync.divergent = విభిన్నం: { $count }: { $ids }

journal.cli.maintenance.sync.divergent.empty = విభిన్నం: 0

journal.cli.maintenance.sync.removed = తీసివేయబడింది: { $count }: { $ids }

journal.cli.maintenance.sync.removed.empty = తీసివేయబడింది: 0

journal.cli.maintenance.sync.synced = సింక్ అయింది: { $count }: { $ids }

journal.cli.maintenance.sync.synced.empty = సింక్ అయింది: 0

journal.cli.maintenance.sync.warning.disabled = హెచ్చరిక: { $id } షెడ్యూల్ డిసేబుల్ అయి ఉంది; డిసేబుల్‌గా ఉంచబడింది

journal.cli.maintenance.sync.warning.divergent = హెచ్చరిక: { $id } షెడ్యూల్ విభిన్నంగా ఉంది; మార్పు లేకుండా ఉంచబడింది

journal.cli.maintenance.trash.erased = ట్రాష్: తుడిచివేసినవి { $erased }, పూర్తయినవి { $finished }, వేచి ఉన్నవి { $waiting }, విఫలమైనవి { $failed }

journal.cli.maintenance.trash.error = ట్రాష్: { $failure }

journal.cli.maintenance.trash.usage = usage: { $id } ఆర్గ్యుమెంట్‌లు తీసుకోదు.

journal.cli.maintenance.usage.face = usage: { $syntax }{ $line_break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: ఊహించని ముఖ సమన్వయ ఆర్గ్యుమెంట్‌లు

journal.cli.maintenance.usage.unknown = usage: { $syntax }{ $line_break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.offload.guidance.changed.retry = బయటికి తరలించడం నడుస్తున్నప్పుడు మీ జర్నల్ మారింది కాబట్టి బయటికి తరలించడం ఆగింది. ఈ కంప్యూటర్ నుండి ఏమీ తీసివేయబడలేదు. దీన్ని మళ్ళీ రన్ చేయండి.

journal.cli.offload.guidance.changed.review = బయటికి తరలించడం నడుస్తున్నప్పుడు మీ జర్నల్ మారింది కాబట్టి బయటికి తరలించడం ఆగింది. ఈ కంప్యూటర్ నుండి ఏమీ తీసివేయబడలేదు. మళ్ళీ ప్రయత్నించే ముందు బయటికి తరలించడం రికవరీ ఫైల్‌లను ఉంచుకోండి మరియు నమోదు అయిన ప్రభావాలను సమీక్షించండి.

journal.cli.offload.guidance.clock = ఆపరేషన్ గడియారాన్ని నిర్ధారించలేకపోయింది.

journal.cli.offload.guidance.journal = మీ జర్నల్‌ను నిర్ధారించలేకపోయింది. సమీక్ష కోసం దాని రికవరీ ఫైల్‌లను ఉంచుకోండి.

journal.cli.offload.guidance.platform = ఈ ప్లాట్‌ఫామ్‌లో బయటికి తరలించడం అమలు అందుబాటులో లేదు.

journal.cli.offload.guidance.start = బయటికి తరలించడం స్టార్ట్ కాలేదు.

journal.cli.offload.guidance.tool = ప్యాకేజీ చేసిన బ్యాకప్ టూల్‌తో ఇన్‌స్టాల్ అయిన { $solstone } కమాండ్‌ను వాడండి.

journal.cli.offload.guidance.unfinished = బయటికి తరలించడం పూర్తి కాలేదు. మళ్ళీ ప్రయత్నించే ముందు దాని రికవరీ ఫైల్‌లను ఉంచుకోండి మరియు నమోదు అయిన ప్రభావాలను సమీక్షించండి.

journal.cli.offload.result.archive.failed = బయటికి తరలించడం ఆర్కైవ్ దశ పూర్తి కాలేదు

journal.cli.offload.result.backup.failing = మీ చివరి బ్యాకప్ విజయవంతంగా పూర్తి కాలేదు

journal.cli.offload.result.backup.not.ready = మీ బ్యాకప్ సెటప్ కాలేదు లేదా ఇంకా విజయవంతమైన రన్ పూర్తి కాలేదు

journal.cli.offload.result.budget.already.satisfied = బయటికి తరలించడం బడ్జెట్ ఇప్పటికే తీరింది, కాబట్టి విడుదల కోసం ఏదీ గుర్తు పెట్టాల్సిన పని లేదు

journal.cli.offload.result.budget.satisfied = బయటికి తరలించడం బడ్జెట్ తీరేంత మీడియా విడుదల కోసం గుర్తు పెట్టబడింది

journal.cli.offload.result.confirm.failed = ఆర్కైవ్ అయిన ఫైల్‌లు నిజంగా బ్యాకప్‌కు చేరాయని బయటికి తరలించడం నిర్ధారించలేకపోయింది

journal.cli.offload.result.confirm.tool.failed = బయటికి తరలించడం ఆర్కైవ్‌ను నిర్ధారించే సరిచూపు పూర్తి కాలేకపోయింది

journal.cli.offload.result.markable.media.exhausted = అర్హమైన ప్రతి ఫైల్ ఇప్పటికే సరిచూడబడింది; ఇప్పుడు విడుదల కోసం ఇంకేమీ గుర్తు పెట్టలేం

journal.cli.offload.result.other = కారణం లేకుండా ఆగింది ({ $reason })

journal.cli.offload.result.unexpected.error = బయటికి తరలించడానికి ఊహించని అంతరాయం కలిగింది

journal.cli.offload.result.verification.failed = చివరి బ్యాకప్ సరిచూపు బ్యాకప్ సమగ్రతలో ఒక సమస్యను కనుగొంది

journal.cli.offload.result.verification.missing = ఇంకా ఏ బ్యాకప్ సరిచూపూ పూర్తి కాలేదు

journal.cli.offload.result.verification.overdue = బ్యాకప్ సరిచూపు ఇటీవల సరిపడినంత విజయవంతం కాలేదు

journal.cli.offload.stopped.archive.cancelled = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ ఆపరేషన్ రద్దు అయింది.

journal.cli.offload.stopped.archive.clock.changed = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ గడియారం వెనక్కి కదిలింది.

journal.cli.offload.stopped.archive.configuration.changed = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ కాన్ఫిగ్ మారింది.

journal.cli.offload.stopped.archive.configuration.unavailable = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ కాన్ఫిగ్‌ను చదవలేకపోయింది.

journal.cli.offload.stopped.archive.credentials.expired = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ క్రెడెన్షియల్స్ గడువు అయ్యాయి.

journal.cli.offload.stopped.archive.deadline.exceeded = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ ఆపరేషన్ డెడ్‌లైన్ చేరింది.

journal.cli.offload.stopped.archive.invalid.credential.expiry = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ క్రెడెన్షియల్స్‌కు చెల్లని గడువు ఉంది.

journal.cli.offload.stopped.archive.invalid.selection = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ మూలాలు ఖాళీ కాని అబ్సల్యూట్ పాత్‌ల ఎంపిక అయి ఉండాలి.

journal.cli.offload.stopped.archive.journal.identity.changed = బయటికి తరలించడం ఆగింది: మీ జర్నల్‌ను నిర్ధారించలేకపోయింది.

journal.cli.offload.stopped.archive.not.configured = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ బ్యాకప్ కాన్ఫిగ్ కాలేదు.

journal.cli.offload.stopped.archive.repository.identity.changed = బయటికి తరలించడం ఆగింది: బ్యాకప్ ఇక సరిపోదు.

journal.cli.offload.stopped.archive.repository.identity.invalid = బయటికి తరలించడం ఆగింది: బ్యాకప్‌ను గుర్తించలేకపోయింది.

journal.cli.offload.stopped.archive.snapshot.identity.invalid = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ పూర్తి స్నాప్‌షాట్ గుర్తింపును ఇవ్వలేదు.

journal.cli.offload.stopped.archive.source.unavailable = బయటికి తరలించడం ఆగింది: సిద్ధం చేసిన ఆర్కైవ్ ఫైల్‌లను నిర్ధారించలేకపోయింది.

journal.cli.offload.stopped.archive.tool.unavailable = బయటికి తరలించడం ఆగింది: ఒక ఆర్కైవ్ టూల్ అందుబాటులో లేదు.

journal.cli.offload.stopped.creation = బయటికి తరలించడం ఆగింది: ప్రపోజల్‌లు చేయడం పూర్తి కాలేదు.

journal.cli.offload.stopped.inputs.io = బయటికి తరలించడం ఆగింది: మీ జర్నల్‌లోని ఒక ఫైల్‌ను తెరవలేకపోయింది లేదా చదవలేకపోయింది.

journal.cli.offload.stopped.inputs.space = బయటికి తరలించడం ఆగింది: ఈ కంప్యూటర్‌లో ఖాళీ చోటు సరిపోదు ({ $available } ఖాళీ, { $required } అవసరం).

journal.cli.offload.stopped.native.start = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ స్టార్ట్ నిరాకరించబడింది (స్థితి { $status }).

journal.cli.offload.stopped.plan.incomplete = బయటికి తరలించడం ఆగింది: ప్రపోజల్‌లను సిద్ధం చేయడం పూర్తి కాలేదు.

journal.cli.offload.stopped.planning = బయటికి తరలించడం ఆగింది: బయటికి తరలించడం తన ప్రపోజల్‌లను సిద్ధం చేయలేకపోయింది.

journal.cli.offload.stopped.readback = బయటికి తరలించడం ఆగింది: మీ బ్యాకప్ నుండి కాపీని తిరిగి చదవడం విఫలమైంది.

journal.cli.offload.stopped.readback.credentials.unavailable = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ చదివే క్రెడెన్షియల్స్ అందుబాటులో లేవు.

journal.cli.offload.stopped.readback.incomplete.discovery = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ వాక్ ప్రతి నోడ్‌ను లెక్కలోకి తీసుకోలేదు.

journal.cli.offload.stopped.readback.invalid.selection = బయటికి తరలించడం ఆగింది: ఎంచుకున్న ఆర్కైవ్ ఫైల్ చెల్లదు.

journal.cli.offload.stopped.readback.native.failure = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ అయిన ఫైల్‌ను సిద్ధం చేయలేకపోయింది.

journal.cli.offload.stopped.readback.not.configured = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ అయిన ఫైల్‌లను చదవడం కాన్ఫిగ్ కాలేదు.

journal.cli.offload.stopped.readback.snapshot.node.unavailable = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ అయిన ఫైల్‌ను నిర్ధారించలేకపోయింది.

journal.cli.offload.stopped.readback.target.root.changed = బయటికి తరలించడం ఆగింది: మీ జర్నల్‌లో ఫైల్‌ను సిద్ధం చేయలేకపోయింది.

journal.cli.offload.stopped.readback.tool.changed = బయటికి తరలించడం ఆగింది: ఇన్‌స్టాల్ అయిన ఆర్కైవ్ టూల్ మారింది; మళ్ళీ ప్రయత్నించండి.

journal.cli.offload.stopped.readback.tool.failed = బయటికి తరలించడం ఆగింది: ఆర్కైవ్ చదివే కమాండ్ విఫలమైంది.

journal.cli.offload.stopped.readback.tool.unavailable = బయటికి తరలించడం ఆగింది: ఇన్‌స్టాల్ అయిన ఆర్కైవ్ టూల్ అందుబాటులో లేదు.

journal.cli.offload.stopped.readback.unsupported = బయటికి తరలించడం ఆగింది: ఈ ప్లాట్‌ఫామ్‌లో ఆర్కైవ్ అయిన ఫైల్‌లను చదవడం అందుబాటులో లేదు.

journal.cli.offload.stopped.readback.unsupported.backend = బయటికి తరలించడం ఆగింది: ఈ బ్యాకప్ గమ్యం ఆర్కైవ్ అయిన ఫైల్‌లను చదవడానికి మద్దతు లేదు.

journal.cli.offload.stopped.readback.unsupported.tool.version = బయటికి తరలించడం ఆగింది: ఇన్‌స్టాల్ అయిన ఆర్కైవ్ టూల్ వెర్షన్ మద్దతు లేనిది.

journal.cli.offload.stopped.record = బయటికి తరలించడం ఆగింది: బయటికి తరలించడం నమోదును సేవ్ చేయడం విఫలమైంది.

journal.cli.offload.stopped.selection = బయటికి తరలించడం ఆగింది: ఎంచుకున్న రికార్డింగ్‌లను నిర్ధారించలేకపోయింది.

journal.cli.offload.stopped.temporary.copy = బయటికి తరలించడం ఆగింది: తాత్కాలిక కాపీ చేయడం విఫలమైంది.

journal.cli.offload.stopped.tool.reason = బయటికి తరలించడం ఆగింది: { $reason }.

journal.cli.offload.stopped.unfinished.work = బయటికి తరలించడం ఆగింది: మీ జర్నల్‌లో ఇంకా పూర్తి కాని పని ఉంది.

journal.cli.reprocess.empty.range = { $day } నుండి { $through } వరకు ఉన్న రోజులకు డేటా లేదు

journal.cli.reprocess.facet.classification = { $path }: చెల్లని ఫాసెట్ వర్గీకరణ

journal.cli.reprocess.facet.utf8 = { $encoding } కాని ఫాసెట్

journal.cli.reprocess.help = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $line_break }గత జర్నల్ రోజును మళ్ళీ ప్రాసెస్ చేయడానికి సమర్పించండి{ $line_break }{ $line_break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $line_break }{ $day_column }{ $date_format } ఫార్మాట్‌లో గత రోజు{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_9 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $through_option }{ $gap_2 }{ $date_format } ఫార్మాట్‌లో కలుపుకొని పరిధి ముగింపు{ $line_break }{ $syntax_continuation_13 }{ $line_break }{ $indent_2 }{ $unit_option }{ $gap_8 }విఫలమైన ఒక రోజువారీ ప్రాసెసింగ్ దశకు మళ్ళీ ప్రయత్నం అభ్యర్థించండి{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_6 }ఆ యూనిట్ ఫాసెట్; యూనిట్‌కు ఫాసెట్ ఉన్నప్పుడు తప్పనిసరి{ $line_break }{ $indent_2 }{ $from_scratch_option }{ $gap_5 }బలవంతంగా మొత్తం రోజువారీ మళ్ళీ రన్, గుర్తులను కాపాడుతూ (ఇది{ $line_break }                     రోజును అప్‌డేట్ అయినట్టు గుర్తు పెట్టదు){ $line_break }{ $indent_2 }{ $mark_updated_option }{ $gap_5 }కొత్త ముడి డేటా ఉందని రోజును గుర్తు పెట్టండి, రోజువారీ ప్రాసెసింగ్{ $line_break }                     దాన్ని మళ్ళీ వరుసలో పెడుతుంది, ఆపై ఖాళీ చేయడాన్ని నడ్జ్ చేయండి{ $line_break }{ $indent_2 }{ $owed_option }{ $gap_13 }రోజు లేదా పరిధిలో బాకీ రోజువారీ అవుట్‌పుట్‌లను, ఎందుకో సహా, జాబితా చేయండి,{ $line_break }                     ఏమీ సమర్పించకుండా{ $line_break }{ $indent_2 }{ $json_option }{ $gap_13 }{ $owed_flag } తో ప్రతి రోజును స్థితులుగా, సంఖ్యలుగా నివేదించండి: దాని{ $line_break }                     గుర్తులు, దాని కవరేజ్, ఎన్ని రోజువారీ అవుట్‌పుట్‌లు ఉన్నాయి,{ $line_break }                     ఎన్ని బాకీ, కారణం వారీగా{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_6 }వివరంగా అవుట్‌పుట్ ఎనేబుల్ చేయండి{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_8 }డీబగ్ లాగింగ్ ఎనేబుల్ చేయండి

journal.cli.reprocess.input.day = చెల్లని రోజు: { $day }

journal.cli.reprocess.intent.busy = అభ్యర్థించిన రోజు మరో ఎగ్జిక్యూటర్ వద్ద ఉంది

journal.cli.reprocess.intent.invalid.day = చెల్లని పని రోజు

journal.cli.reprocess.intent.uncertain = పని ఉద్దేశం ప్రచురణ ఖచ్చితం కాదు: { $outcome }

journal.cli.reprocess.intent.unrecognized = గుర్తించని పని ఉద్దేశం

journal.cli.reprocess.native.arguments = స్థానిక { $binary } { $verb } విఫలమైంది: ఆర్గ్యుమెంట్‌లు చెల్లుబాటు అయ్యే { $encoding } కావు

journal.cli.reprocess.no.data = రోజు { $day } కు డేటా లేదు

journal.cli.reprocess.no.engine = ఇంకా మోడల్ ఎంచుకోలేదు. ఆలోచనలో ఒకటి ఎంచుకోండి, ఆపై మళ్ళీ ప్రయత్నించండి

journal.cli.reprocess.owed.internal = { $flag } నివేదించడమే; { $function } వాడండి

journal.cli.reprocess.owed.only = { $owed_flag } నివేదించడమే; ఏమీ సమర్పించబడలేదు

journal.cli.reprocess.past.only = మళ్ళీ ప్రాసెస్ గత రోజులకే (ఈ రోజు లేదా భవిష్యత్ రోజును మళ్ళీ ప్రాసెస్ చేయలేం)

journal.cli.reprocess.path.utf8 = { $encoding } కాని మూలం పాత్

journal.cli.reprocess.range.plan = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] మొదటి నుండి మళ్ళీ ప్రాసెస్ ప్లాన్:{ $break }డేటా ఉన్న { $days } రోజు ({ $segments } సెగ్మెంట్) వరుసలో పెట్టబడుతుంది. ప్రోగ్రెస్ { $top } లేదా { $health } లో కనిపిస్తుంది. అభ్యర్థనలు సూపర్‌వైజర్ రీస్టార్ట్ తర్వాతా ఉంటాయి; మొదటి నుండి పూర్తి చేయలేని రోజు సాధారణ మళ్ళీ ప్రాసెస్‌గా పూర్తవుతుంది.{ $break }ఈ రోజు గంటలు పట్టవచ్చు; పనుల మధ్య ఈ రోజు పని ముందు వెళ్తుంది.{ $break }కొనసాగడానికి { $yes } తో మళ్ళీ రన్ చేయండి
       *[other] మొదటి నుండి మళ్ళీ ప్రాసెస్ ప్లాన్:{ $break }డేటా ఉన్న { $days } రోజు ({ $segments } సెగ్మెంట్‌లు) వరుసలో పెట్టబడుతుంది. ప్రోగ్రెస్ { $top } లేదా { $health } లో కనిపిస్తుంది. అభ్యర్థనలు సూపర్‌వైజర్ రీస్టార్ట్ తర్వాతా ఉంటాయి; మొదటి నుండి పూర్తి చేయలేని రోజు సాధారణ మళ్ళీ ప్రాసెస్‌గా పూర్తవుతుంది.{ $break }ఈ రోజు గంటలు పట్టవచ్చు; పనుల మధ్య ఈ రోజు పని ముందు వెళ్తుంది.{ $break }కొనసాగడానికి { $yes } తో మళ్ళీ రన్ చేయండి
    }
   *[other] { $segments_quantity ->
        [one] మొదటి నుండి మళ్ళీ ప్రాసెస్ ప్లాన్:{ $break }డేటా ఉన్న { $days } రోజులు ({ $segments } సెగ్మెంట్) వరుసలో పెట్టబడతాయి. ప్రోగ్రెస్ { $top } లేదా { $health } లో కనిపిస్తుంది. అభ్యర్థనలు సూపర్‌వైజర్ రీస్టార్ట్ తర్వాతా ఉంటాయి; మొదటి నుండి పూర్తి చేయలేని రోజు సాధారణ మళ్ళీ ప్రాసెస్‌గా పూర్తవుతుంది.{ $break }ఈ రోజులు గంటలు పట్టవచ్చు; పనుల మధ్య ఈ రోజు పని ముందు వెళ్తుంది.{ $break }కొనసాగడానికి { $yes } తో మళ్ళీ రన్ చేయండి
       *[other] మొదటి నుండి మళ్ళీ ప్రాసెస్ ప్లాన్:{ $break }డేటా ఉన్న { $days } రోజులు ({ $segments } సెగ్మెంట్‌లు) వరుసలో పెట్టబడతాయి. ప్రోగ్రెస్ { $top } లేదా { $health } లో కనిపిస్తుంది. అభ్యర్థనలు సూపర్‌వైజర్ రీస్టార్ట్ తర్వాతా ఉంటాయి; మొదటి నుండి పూర్తి చేయలేని రోజు సాధారణ మళ్ళీ ప్రాసెస్‌గా పూర్తవుతుంది.{ $break }ఈ రోజులు గంటలు పట్టవచ్చు; పనుల మధ్య ఈ రోజు పని ముందు వెళ్తుంది.{ $break }కొనసాగడానికి { $yes } తో మళ్ళీ రన్ చేయండి
    }
}

journal.cli.reprocess.range.saved = { $days_quantity ->
    [one] { $segments_quantity ->
        [one] మొదటి నుండి అభ్యర్థనలు { $days } రోజు ({ $segments } సెగ్మెంట్) కోసం సేవ్ అయ్యాయి{ $break }ప్రోగ్రెస్ స్థితిలో కనిపిస్తుంది; ఈ రోజు పని ముందు వెళ్తుంది
       *[other] మొదటి నుండి అభ్యర్థనలు { $days } రోజు ({ $segments } సెగ్మెంట్‌లు) కోసం సేవ్ అయ్యాయి{ $break }ప్రోగ్రెస్ స్థితిలో కనిపిస్తుంది; ఈ రోజు పని ముందు వెళ్తుంది
    }
   *[other] { $segments_quantity ->
        [one] మొదటి నుండి అభ్యర్థనలు { $days } రోజులు ({ $segments } సెగ్మెంట్) కోసం సేవ్ అయ్యాయి{ $break }ప్రోగ్రెస్ స్థితిలో కనిపిస్తుంది; ఈ రోజు పని ముందు వెళ్తుంది
       *[other] మొదటి నుండి అభ్యర్థనలు { $days } రోజులు ({ $segments } సెగ్మెంట్‌లు) కోసం సేవ్ అయ్యాయి{ $break }ప్రోగ్రెస్ స్థితిలో కనిపిస్తుంది; ఈ రోజు పని ముందు వెళ్తుంది
    }
}

journal.cli.reprocess.record.invalid = ఆకృతి తప్పు రోజువారీ యూనిట్ నమోదు: చెల్లని రోజువారీ యూనిట్ గుర్తింపు

journal.cli.reprocess.record.io = I/O లోపం: { $cause }

journal.cli.reprocess.record.mismatch = ఆకృతి తప్పు రోజువారీ యూనిట్ నమోదు: { $path } వద్ద నమోదులో గుర్తింపు సరిపోలేదు: అవసరం { $expected }, కనబడింది { $found }

journal.cli.reprocess.retry.accepted = ఈ రోజువారీ అవుట్‌పుట్ ఇప్పటికే అంగీకరించబడింది

journal.cli.reprocess.retry.disabled = రోజువారీ టాలెంట్ ఇక ఎనేబుల్ అయి లేదు

journal.cli.reprocess.retry.invalid.day = చెల్లని మళ్ళీ ప్రయత్నం రోజు

journal.cli.reprocess.retry.invalid.unit = చెల్లని మళ్ళీ ప్రయత్నం యూనిట్

journal.cli.reprocess.retry.missing = రోజువారీ యూనిట్ లేదు

journal.cli.reprocess.retry.missing.evidence = నిలిచి ఉన్న ఆధారం లేదు

journal.cli.reprocess.retry.no.longer.held = యూనిట్ ఇక నిలిచి లేదు

journal.cli.reprocess.retry.not.daily = రోజువారీ పని కాదు

journal.cli.reprocess.retry.state = దాని ప్రస్తుత స్థితిని పరిష్కరించకుండా ఈ రోజువారీ యూనిట్‌ను మళ్ళీ ప్రయత్నించలేం

journal.cli.reprocess.schedule.lookback = రోజువారీ షెడ్యూల్ వెనక్కి చూపు { $range } బయట ఉంది

journal.cli.reprocess.schema.missing = స్కీమా ప్రాంప్ట్ పాత్ లేదు

journal.cli.reprocess.schema.name = స్కీమా ఫైల్ పేరు అయి ఉండాలి

journal.cli.reprocess.shape.invalid = { $path } కోసం చెల్లని ఆకారం

journal.cli.reprocess.shape.object = ఆకారం sidecar ఆబ్జెక్ట్ కాదు

journal.cli.reprocess.shape.written = చెల్లని రాసిన మూలం ఆకారం: { $path }

journal.cli.reprocess.source.json.object = { $path } { $encoding } ఆబ్జెక్ట్ కాదు

journal.cli.reprocess.source.line.error = { $path } పంక్తి { $line }: { $error }

journal.cli.reprocess.source.line.object = { $path } పంక్తి { $line } ఆబ్జెక్ట్ కాదు

journal.cli.reprocess.source.symlink = రోజువారీ మూలం సింలింక్: { $path }

journal.cli.reprocess.source.utf8 = { $encoding } కాని రోజువారీ మూలం పాత్

journal.cli.reprocess.talent.overrides = { $path } నుండి టాలెంట్ ఓవర్‌రైడ్‌లు చదవలేకపోయింది

journal.cli.reprocess.template.utf8 = { $encoding } కాని టెంప్లేట్ పేరు

journal.cli.reprocess.through.order = { $through_flag } ప్రారంభ రోజు నాడు లేదా ఆ తర్వాత అయి ఉండాలి

journal.cli.reprocess.through.required = { $through_flag } కు { $from_scratch_flag } అవసరం

journal.cli.reprocess.unit.failed = మళ్ళీ ప్రాసెస్ యూనిట్ విఫలమైంది: { $error }

journal.cli.reprocess.unit.saved = { $unit } కోసం { $day } రోజున మళ్ళీ ప్రయత్నం అభ్యర్థన సేవ్ అయింది

journal.cli.reprocess.unreadable = { $day }  (చదవలేనిది: { $error })

journal.cli.reprocess.usage.conflict = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: లోపం: ఆర్గ్యుమెంట్ { $option }: ఆర్గ్యుమెంట్ { $previous } తో అనుమతి లేదు

journal.cli.reprocess.usage.missing.day = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $positional }

journal.cli.reprocess.usage.missing.value = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: లోపం: ఆర్గ్యుమెంట్ { $option }: ఒక ఆర్గ్యుమెంట్ అవసరం

journal.cli.reprocess.usage.requires = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: లోపం: ఆర్గ్యుమెంట్ { $option }: { $required } అవసరం

journal.cli.reprocess.usage.unknown = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $day_line }{ $line_break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.reprocess.waiting = అభ్యర్థన సేవ్ అయింది; దాన్ని ప్రాసెస్ చేయడానికి { $start_command } తో బ్యాక్‌గ్రౌండ్ సేవను స్టార్ట్ చేయండి

journal.cli.retention.proposal.eligible = పాలసీ అర్హత: అర్హం { $open_brace } యాంకర్: { $anchor }, age_days: { $age_days }, కాలం: రోజులు({ $period_days }) { $close_brace }

journal.cli.retention.proposal.forever = పాలసీ అర్హత: ఎప్పటికీ ఉంచబడుతుంది

journal.cli.retention.proposal.no.anchor = పాలసీ అర్హత: యాంకర్ లేదు { $open_brace } యాంకర్: { $anchor } { $close_brace }

journal.cli.retention.proposal.unrecorded = ప్రతిపాదన నమోదు కాకముందే తీసివేత విఫలమైంది

journal.cli.retention.proposal.young = పాలసీ అర్హత: మరీ తక్కువ వయసు { $open_brace } యాంకర్: { $anchor }, age_days: { $age_days }, కాలం: రోజులు({ $period_days }) { $close_brace }

journal.cli.retention.recovery.activity.binding = మీ జర్నల్‌ను సరిచూడలేకపోయింది, కాబట్టి ఈ తొలగింపు ఆగింది

journal.cli.retention.recovery.activity.busy = మీ జర్నల్ మరో పనితో బిజీగా ఉంది

journal.cli.retention.recovery.activity.hold = ఈ తీసివేత కొనసాగే ముందు మరో జర్నల్ ఆపరేషన్‌కు శ్రద్ధ కావాలి

journal.cli.retention.recovery.activity.reserved = ఈ స్థానం ఒక జర్నల్ ఆపరేషన్ కోసం రిజర్వ్ అయి ఉంది, దాన్ని తీసివేయలేం

journal.cli.retention.recovery.path.directory = రికార్డింగ్ ఉండాల్సిన చోట ఫోల్డర్ ఉంది

journal.cli.retention.recovery.path.invalid = ఈ స్థానం మీ జర్నల్‌లోని ఫైల్‌ను పేర్కొనదు

journal.cli.retention.recovery.path.outside = ఇది మీ జర్నల్‌లో లేదు, కాబట్టి దాన్ని అలాగే వదిలారు

journal.cli.retention.recovery.path.permission = ఫైల్‌ను చదవలేకపోయింది లేదా తీసివేయలేకపోయింది, ఇది సాధారణంగా అనుమతి సమస్య

journal.cli.retention.recovery.path.unavailable = ఫైల్‌ను తీసివేయలేకపోయింది; జర్నల్ మారకుండా ఉంది

journal.cli.retention.recovery.record.prepare = ఈ తీసివేత నమోదును సిద్ధం చేయలేకపోయింది

journal.cli.retention.recovery.record.write = ఈ తీసివేత నమోదును రాయలేకపోయింది

journal.cli.retention.recovery.segment.busy = మరో ప్రాసెస్ ఈ సెగ్మెంట్ మీద పని చేస్తోంది

journal.cli.retention.recovery.segment.gone = ఈ సెగ్మెంట్ ఇప్పటికే తీసివేయబడింది

journal.cli.retention.recovery.segment.live.occupied = ఈ సెగ్మెంట్ ఉన్న చోట కొత్తది రాయబడింది, కాబట్టి దాన్ని ఓవర్‌రైట్ చేయకుండా పక్కన పెట్టారు

journal.cli.retention.recovery.segment.missing = మీ జర్నల్‌లో అలాంటి సెగ్మెంట్ లేదు

journal.cli.retention.recovery.segment.staged = ఈ సెగ్మెంట్ తీసివేత మధ్యలో ఉంది మరియు పక్కన పెట్టబడింది; అది పూర్తయ్యే వరకు మీ ఇతర సెగ్మెంట్‌లతో జాబితాలో ఉండదు

journal.cli.retention.recovery.segment.unconfirmed = ఇది పోయిందని నిర్ధారించలేకపోయింది, కాబట్టి తీసివేయబడిందని నివేదించడం లేదు

journal.cli.retention.recovery.segment.unfinished = ఈ సెగ్మెంట్ మునుపటి తీసివేత పూర్తి కాలేదు; దీన్ని మళ్ళీ ప్రయత్నించే ముందు దాన్ని చూడాలి

journal.cli.retention.removal.uncertain = తొలగింపు పూర్తి చేయలేకపోయింది లేదా నిర్ధారించలేకపోయింది. రీస్టోర్ అవసరం.

journal.cli.schedule.cell.disabled = డిసేబుల్

journal.cli.schedule.cell.invalid = చెల్లదు

journal.cli.schedule.cell.midnight = అర్ధరాత్రి

journal.cli.schedule.cell.never = ఎప్పుడూ లేదు

journal.cli.schedule.cell.now = ఇప్పుడు

journal.cli.schedule.config = కాన్ఫిగ్: { $path }

journal.cli.schedule.empty = షెడ్యూల్‌లు కాన్ఫిగర్ కాలేదు.{ $line_break }{ $line_break }షెడ్యూల్‌లను ఇక్కడ చేర్చండి: { $path }

journal.cli.schedule.error.cmd.invalid = షెడ్యూల్ '{ $name }' లో చెల్లని cmd ఉంది

journal.cli.schedule.error.config.malformed = { $path } వద్ద షెడ్యూల్‌లు కాన్ఫిగ్ ఆకృతి తప్పు

journal.cli.schedule.error.config.not.object = { $path } వద్ద షెడ్యూల్‌లు కాన్ఫిగ్ JSON ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.schedule.error.config.unreadable = { $path } వద్ద షెడ్యూల్‌లు కాన్ఫిగ్ చదవలేనిది

journal.cli.schedule.error.entry.invalid = షెడ్యూల్ '{ $name }' చెల్లుబాటు అయ్యే ఎంట్రీ కాదు

journal.cli.schedule.error.state.malformed = { $path } వద్ద షెడ్యూలర్ స్థితి ఆకృతి తప్పు

journal.cli.schedule.error.state.not.object = { $path } వద్ద షెడ్యూలర్ స్థితి JSON ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.schedule.error.state.unreadable = { $path } వద్ద షెడ్యూలర్ స్థితి చదవలేనిది

journal.cli.schedule.header.cmd = CMD

journal.cli.schedule.header.every = EVERY

journal.cli.schedule.header.last.run = LAST RUN

journal.cli.schedule.header.name = NAME

journal.cli.schedule.header.next.due = NEXT DUE

journal.cli.schedule.tag.disabled = [డిసేబుల్]

journal.cli.schedule.weekly.friday = శుక్రవారం { $time }

journal.cli.schedule.weekly.monday = సోమవారం { $time }

journal.cli.schedule.weekly.saturday = శనివారం { $time }

journal.cli.schedule.weekly.sunday = ఆదివారం { $time }

journal.cli.schedule.weekly.thursday = గురువారం { $time }

journal.cli.schedule.weekly.tuesday = మంగళవారం { $time }

journal.cli.schedule.weekly.wednesday = బుధవారం { $time }

journal.cli.segment.argument.missing = usage: { $syntax }{ $line_break }{ $command }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $input }

journal.cli.segment.argument.unknown = usage: { $syntax }{ $line_break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.segment.check.backward = వెనుకకు గొలుసు

journal.cli.segment.check.chain.no.day = { $prev_segment_key } సెట్ అయి ఉంది, { $prev_day_key } లేకుండా

journal.cli.segment.check.chain.no.successor = తర్వాతి సెగ్మెంట్ కనబడలేదు, ప్రవాహం చివర కాదు

journal.cli.segment.check.chain.previous = మునుపటి సెగ్మెంట్ కనబడింది

journal.cli.segment.check.chain.start = మునుపటి సెగ్మెంట్ లేదు

journal.cli.segment.check.chain.tail = ప్రవాహం చివర

journal.cli.segment.check.content.empty = { $audio_file }, { $screen_file } లేదా { $browser_files } లేదు

journal.cli.segment.check.content.present = కంటెంట్ ఫైల్‌లు ఉన్నాయి

journal.cli.segment.check.directory.absent = సెగ్మెంట్ డైరెక్టరీ లేదు

journal.cli.segment.check.directory.exists = డైరెక్టరీ ఉంది

journal.cli.segment.check.directory.missing = డైరెక్టరీ లేదు

journal.cli.segment.check.forward = ముందుకు గొలుసు

journal.cli.segment.check.index = ఇండెక్స్ ఉనికి

journal.cli.segment.check.index.absent = జర్నల్ ఇండెక్స్ అందుబాటులో లేదు

journal.cli.segment.check.index.error = జర్నల్ ఇండెక్స్ లోపం: { $error } (రన్ చేయండి: { $index_command })

journal.cli.segment.check.index.missing = సెగ్మెంట్ ఇండెక్స్ కాలేదు

journal.cli.segment.check.index.present = సెగ్మెంట్ ఇండెక్స్ అయి ఉంది

journal.cli.segment.check.marker.exists = { $stream_file } ఉంది

journal.cli.segment.check.marker.invalid = { $stream_file } చెల్లని { $json_format }

journal.cli.segment.check.marker.missing = { $stream_file } లేదు

journal.cli.segment.check.marker.no.stream = { $stream_file } లో ప్రవాహం ఫీల్డ్ లేదు

journal.cli.segment.check.marker.valid = { $stream_file } చెల్లుబాటు అయ్యేది

journal.cli.segment.check.next = తర్వాతి సెగ్మెంట్ { $path }

journal.cli.segment.check.previous.missing = మునుపటి సెగ్మెంట్ { $path } లేదు

journal.cli.segment.day.empty = { $day } కోసం సెగ్మెంట్‌లు కనబడలేదు

journal.cli.segment.guard.activity = ఈ మార్పు కొనసాగే ముందు మరో జర్నల్ ఆపరేషన్‌కు శ్రద్ధ కావాలి

journal.cli.segment.guard.binding = జర్నల్‌ను సరిచూడలేకపోయింది, కాబట్టి ఈ మార్పు నిలిచి ఉంది

journal.cli.segment.guard.reserved = ఈ స్థానం ఒక జర్నల్ ఆపరేషన్ కోసం రిజర్వ్ అయి ఉంది, దాన్ని మార్చలేం

journal.cli.segment.help.inspect = usage: { $syntax }{ $break }{ $break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $break }{ $option_3 }సెగ్మెంట్ పాత్: { $example_path } (ఉదా.{ $break }{ $syntax_line_4 }{ $break }{ $break }ఆప్షన్‌లు:{ $break }{ $option_7 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $break }{ $option_8 }{ $json_format } గా అవుట్‌పుట్

journal.cli.segment.help.list = usage: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $break }{ $option_5 }{ $day_format } ఫార్మాట్‌లో రోజు{ $break }{ $break }ఆప్షన్‌లు:{ $break }{ $option_8 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $break }{ $option_9 }నిర్దిష్ట ప్రవాహానికి ఫిల్టర్ చేయండి{ $break }{ $option_10 }{ $json_format } గా అవుట్‌పుట్

journal.cli.segment.help.move = usage: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $syntax_line_2 }{ $break }{ $syntax_line_3 }{ $break }{ $break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $break }{ $option_6 }సెగ్మెంట్ పాత్: { $example_path } (ఉదా.{ $break }{ $syntax_line_7 }{ $break }{ $break }ఆప్షన్‌లు:{ $break }{ $option_10 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $break }{ $option_11 }గమ్యం రోజు ({ $day_format }){ $break }{ $option_12 }కొత్త సమయం ({ $time_format }), అసలు వ్యవధిని ఉంచుతూ{ $break }{ $option_13 }మార్పులు చేయకుండా ప్లాన్ చూపండి{ $break }{ $option_14 }వివరమైన ప్రోగ్రెస్ చూపండి

journal.cli.segment.help.top = usage: { $syntax }{ $break }{ $break }జర్నల్ సెగ్మెంట్‌లను పరిశీలించి నిర్వహించండి{ $break }{ $break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $break }{ $syntax_line_5 }{ $break }{ $option_6 }ఒక రోజు సెగ్మెంట్‌లను జాబితా చేయండి{ $break }{ $option_7 }సెగ్మెంట్ మెటాడేటా చూపండి{ $break }{ $option_8 }సెగ్మెంట్ సమగ్రతను ధృవీకరించండి{ $break }{ $option_9 }సెగ్మెంట్‌ను వేరే రోజు/సమయానికి తరలించండి{ $break }{ $break }ఆప్షన్‌లు:{ $break }{ $option_12 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $break }{ $option_13 }వివరంగా అవుట్‌పుట్ ఎనేబుల్ చేయండి{ $break }{ $option_14 }డీబగ్ లాగింగ్ ఎనేబుల్ చేయండి

journal.cli.segment.help.verify = usage: { $syntax }{ $break }{ $syntax_line_1 }{ $break }{ $break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $break }{ $option_4 }సెగ్మెంట్ పాత్: { $example_path }{ $break }{ $break }ఆప్షన్‌లు:{ $break }{ $option_7 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $break }{ $option_8 }ఒక రోజు సెగ్మెంట్‌లన్నీ ధృవీకరించండి{ $break }{ $option_9 }{ $json_format } గా అవుట్‌పుట్

journal.cli.segment.identity.default = సెగ్మెంట్ జాబితా నిరాకరించబడింది: పేరు ఉన్న ప్రవాహ డైరెక్టరీ "{ $default_stream }" ను నమోదు గుర్తింపుగా రాయలేం: { $path }

journal.cli.segment.identity.duplicate = సెగ్మెంట్ జాబితా నిరాకరించబడింది: పలు సెగ్మెంట్‌లు ప్రవాహం { $stream } కీ { $key } ను పంచుకుంటాయి

journal.cli.segment.identity.encoding = సెగ్మెంట్ జాబితా నిరాకరించబడింది: సెగ్మెంట్ పాత్ { $encoding } లో సూచించలేం: { $path }

journal.cli.segment.identity.refused = సెగ్మెంట్ జాబితా నిరాకరించబడింది: { $detail }

journal.cli.segment.inspect.chain = గొలుసు:

journal.cli.segment.inspect.events = { $quantity ->
    [one] ఈవెంట్‌లు: { $entries } ఎంట్రీ
   *[other] ఈవెంట్‌లు: { $entries } ఎంట్రీలు
}

journal.cli.segment.inspect.events.tracts = { $quantity ->
    [one] ఈవెంట్‌లు: { $entries } ఎంట్రీ ({ $tracts })
   *[other] ఈవెంట్‌లు: { $entries } ఎంట్రీలు ({ $tracts })
}

journal.cli.segment.inspect.files = ఫైల్‌లు ({ $count }):

journal.cli.segment.inspect.index.error = ఇండెక్స్: లోపం ({ $error }) - రన్ చేయండి: { $index_command }

journal.cli.segment.inspect.index.missing = ఇండెక్స్: ఇండెక్స్ కాలేదు

journal.cli.segment.inspect.index.unavailable = ఇండెక్స్: అందుబాటులో లేదు

journal.cli.segment.inspect.indexed = { $quantity ->
    [one] ఇండెక్స్: ఇండెక్స్ అయి ఉంది ({ $chunks } చంక్)
   *[other] ఇండెక్స్: ఇండెక్స్ అయి ఉంది ({ $chunks } చంక్‌లు)
}

journal.cli.segment.inspect.next = తర్వాత: { $next }

journal.cli.segment.inspect.next.none = తర్వాత: (ఏమీ లేదు)

journal.cli.segment.inspect.next.tail = తర్వాత: (చివర)

journal.cli.segment.inspect.path = సెగ్మెంట్: { $path }

journal.cli.segment.inspect.prev = మునుపటి: { $previous }

journal.cli.segment.inspect.prev.missing = మునుపటి: { $path } [లేదు]

journal.cli.segment.inspect.prev.none = మునుపటి: (ఏమీ లేదు)

journal.cli.segment.inspect.size = పరిమాణం: { $size }

journal.cli.segment.inspect.stream = ప్రవాహం:  { $stream }

journal.cli.segment.inspect.stream.seq = ప్రవాహం:  { $stream } (సీక్వెన్స్ { $sequence })

journal.cli.segment.inspect.talents = టాలెంట్‌లు ({ $count }):

journal.cli.segment.inspect.time = సమయం:    { $range } ({ $seconds } సె)

journal.cli.segment.list.heading.dur = DUR

journal.cli.segment.list.heading.files = FILES

journal.cli.segment.list.heading.segment = SEGMENT

journal.cli.segment.list.heading.size = SIZE

journal.cli.segment.list.heading.stream = STREAM

journal.cli.segment.list.heading.talents = TALENTS

journal.cli.segment.list.heading.time = TIME

journal.cli.segment.missing = సెగ్మెంట్ కనబడలేదు: { $path }

journal.cli.segment.move.day.format = చెల్లని { $to_day_flag } ఫార్మాట్: { $to_day } (అవసరం { $date_format })

journal.cli.segment.move.directory.created = డైరెక్టరీ సృష్టించబడింది: { $path }

journal.cli.segment.move.directory.exists = గమ్యం { $destination } ఇప్పటికే ఉంది; మార్పులు ఏమీ చేయలేదు

journal.cli.segment.move.directory.failed = దశ 1 డైరెక్టరీ తరలింపు విఫలమైంది: { $error }; మూలం మరియు ఇండెక్స్ అధికారికంగా ఉన్నాయి

journal.cli.segment.move.directory.moved = డైరెక్టరీ తరలించబడింది: { $source } -> { $destination }

journal.cli.segment.move.dry = [డ్రై-రన్] మార్పులు ఏమీ చేయలేదు

journal.cli.segment.move.events.none = మళ్ళీ రాయడానికి { $event_file } లేదు

journal.cli.segment.move.events.rewritten = { $quantity ->
    [one] { $count } { $event_file } పంక్తి మళ్ళీ రాయబడింది (రోజు: { $source_day }->{ $destination_day }, సెగ్మెంట్: { $source }->{ $destination })
   *[other] { $count } { $event_file } పంక్తులు మళ్ళీ రాయబడ్డాయి (రోజు: { $source_day }->{ $destination_day }, సెగ్మెంట్: { $source }->{ $destination })
}

journal.cli.segment.move.exists = సెగ్మెంట్ { $segment } ఇప్పటికే { $day }/{ $stream } మీద ఉంది. వేరే సమయం పేర్కొనడానికి { $to_time_flag } వాడండి.

journal.cli.segment.move.fail.ambiguous = { $quantity ->
    [one] దశ 3 తర్వాతి నమోదు ప్యాచ్ తరలింపు తర్వాత అసంగతం: అయోమయ గొలుసు: { $count } తర్వాతి నమోదు ప్యాచ్ కాలేదు
   *[other] దశ 3 తర్వాతి నమోదు ప్యాచ్ తరలింపు తర్వాత అసంగతం: అయోమయ గొలుసు: { $count } తర్వాతి నమోదులు ప్యాచ్ కాలేదు
}

journal.cli.segment.move.fail.events = దశ 2 ఈవెంట్‌లు మళ్ళీ రాత తరలింపు తర్వాత అసంగతం: { $detail }

journal.cli.segment.move.fail.health.marker = దశ 6 స్థితి గుర్తు తాకడం తరలింపు తర్వాత అసంగతం: { $detail }

journal.cli.segment.move.fail.search = దశ 5 సెర్చ్ అప్‌డేట్ తరలింపు తర్వాత అసంగతం: సెర్చ్ అప్‌డేట్‌ను వరుసలో పెట్టలేకపోయింది: { $error }

journal.cli.segment.move.fail.successor = దశ 3 తర్వాతి నమోదు ప్యాచ్ తరలింపు తర్వాత అసంగతం: { $detail }

journal.cli.segment.move.fail.tail.locked = దశ 4 ప్రవాహం చివర బాగుచేయడం తరలింపు తర్వాత అసంగతం: ప్రవాహం నమోదును లాక్ చేయలేకపోయింది

journal.cli.segment.move.fail.tail.malformed = దశ 4 ప్రవాహం చివర బాగుచేయడం తరలింపు తర్వాత అసంగతం: ఆకృతి తప్పు ప్రవాహం నమోదు

journal.cli.segment.move.fail.tail.no.record = దశ 4 ప్రవాహం చివర బాగుచేయడం తరలింపు తర్వాత అసంగతం: ప్రవాహం నమోదు లేదు; గుర్తింపు మళ్ళీ రాయబడలేదు

journal.cli.segment.move.fail.tail.write = దశ 4 ప్రవాహం చివర బాగుచేయడం తరలింపు తర్వాత అసంగతం: అటామిక్ ప్రచురణ విఫలమైంది

journal.cli.segment.move.markers.touched = తాకిన స్థితి గుర్తులు: { $days }

journal.cli.segment.move.no.duration = మూలం సెగ్మెంట్ కీకు వ్యవధి లేదు

journal.cli.segment.move.no.marker = మూలం సెగ్మెంట్‌లో { $stream_file } లేదు

journal.cli.segment.move.no.slot = { $destination_segment } దగ్గర { $to_day } రోజున అందుబాటులో ఉన్న సెగ్మెంట్ స్లాట్ లేదు

journal.cli.segment.move.plan = తరలింపు: { $source } -> { $destination }

journal.cli.segment.move.plan.ambiguous = { $quantity ->
    [one] ప్యాచ్ చేయాల్సిన తర్వాతి నమోదు: (అయోమయం: { $count } తర్వాతి నమోదు)
   *[other] ప్యాచ్ చేయాల్సిన తర్వాతి నమోదు: (అయోమయం: { $count } తర్వాతి నమోదులు)
}

journal.cli.segment.move.plan.events = { $event_file } పంక్తులు: { $count }

journal.cli.segment.move.plan.index = ఇండెక్స్ చంక్‌లు: { $chunks }

journal.cli.segment.move.plan.index.error = ఇండెక్స్ చదవడంలో లోపం: { $error } (తొలగించి మళ్ళీ ఇండెక్స్ చేయడం ప్రయత్నించబడుతుంది; రన్ చేయండి: { $index_command })

journal.cli.segment.move.plan.markers = స్థితి గుర్తులు: { $source_day }, { $destination_day }

journal.cli.segment.move.plan.successor = ప్యాచ్ చేయాల్సిన తర్వాతి నమోదు: { $path }

journal.cli.segment.move.plan.tail = ప్యాచ్ చేయాల్సిన తర్వాతి నమోదు: (ఏమీ లేదు - ప్రవాహం చివర)

journal.cli.segment.move.running = తరలింపు జరుగుతోంది...

journal.cli.segment.move.same = మూలం మరియు గమ్యం ఒకటే

journal.cli.segment.move.search.queued = సెర్చ్ అప్‌డేట్‌లు వరుసలో ఉన్నాయి

journal.cli.segment.move.stream.mismatch = ప్రవాహం సరిపోలేదు: పాత్ '{ $path_stream }' అంటోంది కానీ { $stream_file } '{ $marker_stream }' అంటోంది

journal.cli.segment.move.stream.rebuilt = ప్రవాహం స్థితి మళ్ళీ బిల్డ్ అయింది: { $stream }

journal.cli.segment.move.successor.none = ప్యాచ్ చేయాల్సిన తర్వాతి నమోదు లేదు (ప్రవాహం చివర)

journal.cli.segment.move.successor.patched = ప్యాచ్ అయిన తర్వాతి నమోదు { $path }

journal.cli.segment.move.talents = { $command } తాకిన రోజుల మీద రోజువారీ టాలెంట్‌లను మళ్ళీ రన్ చేస్తుంది

journal.cli.segment.move.time.format = చెల్లని { $to_time_flag } ఫార్మాట్: { $time } (అవసరం { $time_format })

journal.cli.segment.path.syntax = సెగ్మెంట్ పాత్ { $path_form } అయి ఉండాలి (ఉదా. { $example })

journal.cli.segment.relocation.deleted = సెగ్మెంట్‌ను మీరు తొలగించారు

journal.cli.segment.relocation.location = సెగ్మెంట్ స్థానం ప్లాన్ చేసిన తరలింపుతో సరిపోలేదు

journal.cli.segment.relocation.marker = గుర్తు చదవలేకపోయింది

journal.cli.segment.status.pass = పాస్ అయింది

journal.cli.segment.verify.day.result = { $quantity ->
    [one] సారాంశం: { $passed }/{ $total } సరిచూపు పాస్ అయింది
   *[other] సారాంశం: { $passed }/{ $total } సరిచూపులు పాస్ అయ్యాయి
}

journal.cli.segment.verify.input = { $verb } కు సెగ్మెంట్ పాత్ లేదా { $day_flag } అవసరం

journal.cli.segment.verify.result = { $quantity ->
    [one] { $passed }/{ $total } సరిచూపు పాస్ అయింది
   *[other] { $passed }/{ $total } సరిచూపులు పాస్ అయ్యాయి
}

journal.cli.share.activities = { $facet } కార్యకలాపాలు { $day } కోసం చదవలేకపోయింది: { $detail }

journal.cli.share.activity.missing = '{ $activity }' కార్యకలాపం ఫాసెట్ { $facet } లో { $day } రోజున లేదు

journal.cli.share.activity.unmatched = కార్యకలాపం '{ $activity }' { $person } తో సరిపోలేదు

journal.cli.share.facets = ఫాసెట్‌లు జాబితా చేయలేకపోయింది: { $detail }

journal.cli.share.failure = { $command }: { $detail }

journal.cli.share.help.match = usage: { $syntax }{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }ఎంటిటీ ఐడి, లేదా ఒక వ్యక్తి ప్రత్యేక పేరు లేదా అలియాస్{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }చదవాల్సిన రోజు{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }ఈ ఫాసెట్ మాత్రమే (డిఫాల్ట్: ప్రతి ఫాసెట్)

journal.cli.share.help.package = usage: { $syntax }{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $person_option }{ $gap_3 }ఎంటిటీ ఐడి, లేదా ఒక వ్యక్తి ప్రత్యేక పేరు లేదా అలియాస్{ $line_break }{ $indent_2 }{ $day_option }{ $gap_4 }కార్యకలాపం రోజు{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_5 }కార్యకలాపం ఫాసెట్{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_5 }కార్యకలాపం ఐడి

journal.cli.share.help.root = usage: { $syntax }{ $line_break }{ $line_break }లింక్ అయిన వ్యక్తితో పంచుకున్న కార్యకలాపాలను కనుగొని, ఏమి పంచుకోబడుతుందో ప్రివ్యూ చూడండి.{ $line_break }రెండు సబ్‌కమాండ్‌లు జర్నల్‌ను మాత్రమే చదువుతాయి.{ $line_break }{ $line_break }కమాండ్‌లు:{ $line_break }{ $match_command }ఒక వ్యక్తితో సరిపోయే ఆ రోజు కార్యకలాపాలను జాబితా చేయండి, ఒక్కో { $json } పంక్తి{ $line_break }{ $package_command }సరిపోయే ఒక కార్యకలాపానికి స్కోప్ చేసిన సారాంశం ప్యాకేజీని ముద్రించండి

journal.cli.share.parse.argument = { $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్ '{ $argument }'

journal.cli.share.parse.duplicate = { $command }: లోపం: { $flag } ఒకటి కంటే ఎక్కువసార్లు ఇవ్వబడింది

journal.cli.share.parse.encoding = { $command }: లోపం: ఆర్గ్యుమెంట్‌లు చెల్లుబాటు అయ్యే { $encoding } అయి ఉండాలి

journal.cli.share.parse.missing = { $command }: లోపం: సబ్‌కమాండ్ తప్పనిసరి

journal.cli.share.parse.required = { $command }: లోపం: { $option } తప్పనిసరి

journal.cli.share.parse.unknown = { $command }: లోపం: తెలియని సబ్‌కమాండ్ '{ $argument }'

journal.cli.share.parse.value = { $command }: లోపం: { $flag } కు విలువ కావాలి

journal.cli.share.provider.activity.unavailable = కార్యకలాపం సారాంశం అందుబాటులో లేదు: { $detail }

journal.cli.share.provider.child.cleanup = { $primary } (క్లీనప్: { $cleanup })

journal.cli.share.provider.child.exit = ఎగ్జిట్ { $code }

journal.cli.share.provider.child.exit.signal = ఎగ్జిట్ { $code } సిగ్నల్ { $signal }

journal.cli.share.provider.child.invalid = చెల్లని చైల్డ్ సమాధానం ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process = ప్రాసెస్ I/O విఫలమైంది: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.process.cleanup = ప్రాసెస్ I/O విఫలమైంది: { $primary }; { $status_field }{ $status }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }; { $cleanup_field }{ $cleanup }

journal.cli.share.provider.child.protocol = ప్రోటోకాల్ { $reason } ({ $status }): { $detail }; { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.child.signal = సిగ్నల్ { $signal }

journal.cli.share.provider.child.status.unknown = స్థితి తెలియదు

journal.cli.share.provider.child.truncated = { $bytes } [కత్తిరించిన]

journal.cli.share.provider.child.unavailable = అందుబాటులో లేదు

journal.cli.share.provider.child.unexpected = ఊహించని చైల్డ్ ({ $status }); { $stdin_closed_early_field }{ $early }; { $stdout_field }{ $out }; { $stderr_field }{ $err }

journal.cli.share.provider.entities.read = ఎంటిటీలు చదవలేకపోయింది: { $detail }

journal.cli.share.provider.entity.invalid = జర్నల్ ఎంటిటీ { $entity } లోడ్ చేయలేకపోయింది: { $detail }

journal.cli.share.provider.matching = { $facet }/{ $day }/{ $activity } సరిపోలిక { $entity } కోసం

journal.cli.share.provider.model.unconfigured = మోడల్ ఇంకా ఎంచుకోలేదు. ఆలోచనలో ఒకదాన్ని ఎంచుకోండి.

journal.cli.share.provider.person.absent = ఈ జర్నల్‌లో '{ $query }' అనే పేరు ఉన్న వ్యక్తి లేరు

journal.cli.share.provider.person.ambiguous = '{ $query }' ఒకటి కంటే ఎక్కువ వ్యక్తులను సూచిస్తుంది ({ $people }); ఎంటిటీ ఐడి వాడండి

journal.cli.share.provider.presence.unavailable = ఉనికి సరిచూపు అందుబాటులో లేదు: { $detail }

journal.cli.share.provider.segment.unavailable = సెగ్మెంట్ సారాంశం అందుబాటులో లేదు: { $detail }

journal.cli.share.provider.talent.invalid = టాలెంట్ '{ $talent }' చెల్లని { $json } ఇచ్చింది: { $error }

journal.cli.share.provider.talent.prepare = టాలెంట్ '{ $talent }': { $detail }

journal.cli.share.provider.talent.timeout.numeric = కాన్ఫిగ్ '{ $talent }' సంఖ్య కాని { $timeout_s_field } ప్రకటిస్తుంది

journal.cli.share.provider.talent.timeout.range = కాన్ఫిగ్ '{ $talent }' { $timeout_s_field } మద్దతు ఉన్న పరిధి బయట ప్రకటిస్తుంది

journal.cli.share.provider.talent.tokens = టాలెంట్ '{ $talent }' { $max_output_tokens_field } లేదు అని ప్రకటిస్తుంది

journal.cli.share.provider.talent.type = మద్దతు లేని టాలెంట్ రకం: { $kind }

journal.cli.share.provider.talent.unfinished = టాలెంట్ '{ $talent }' పూర్తి కాలేదు: { $outcome }

journal.cli.share.startup.failed = { $command } విఫలమైంది: { $detail }

journal.cli.share.startup.runtime.current = వర్కర్ ఎగ్జిక్యూటబుల్ రిజల్వ్ చేయలేకపోయింది: { $error }

journal.cli.share.startup.runtime.parent = వర్కర్ ఎగ్జిక్యూటబుల్‌కు పేరెంట్ లేదు: { $path }

journal.cli.share.startup.runtime.root = ఎగ్జిక్యూటబుల్ డైరెక్టరీ నుండి ఇన్‌స్టాలేషన్ రూట్ రిజల్వ్ చేయలేకపోయింది: { $path }

journal.cli.share.startup.sibling.missing = { $command } విఫలమైంది: సిబ్లింగ్ ఎగ్జిక్యూటబుల్ { $path } లేదు

journal.cli.share.startup.sibling.parent = { $command } విఫలమైంది: ప్రస్తుత ఎగ్జిక్యూటబుల్‌కు పేరెంట్ లేదు

journal.cli.share.transport.child.inspect = ముగింపుకు ముందు చైల్డ్ ప్రాసెస్ పరిశీలన: { $detail }

journal.cli.share.transport.child.reap = చైల్డ్ ప్రాసెస్ రీప్: { $detail }

journal.cli.share.transport.child.retry = చైల్డ్ ప్రాసెస్ రీప్ మళ్ళీ ప్రయత్నం: { $detail }

journal.cli.share.transport.child.terminate = చైల్డ్ ప్రాసెస్ టెర్మినేట్: { $detail }

journal.cli.share.transport.collector = { $pipe } కలెక్టర్: { $detail }

journal.cli.share.transport.collector.panic = { $pipe } కలెక్టర్: థ్రెడ్ పానిక్ అయింది

journal.cli.share.transport.field.array = { $field } అర్రే లేదా null అయి ఉండాలి

journal.cli.share.transport.field.boolean = { $field } బూలియన్ అయి ఉండాలి

journal.cli.share.transport.field.optional = { $field } స్ట్రింగ్ లేదా null అయి ఉండాలి

journal.cli.share.transport.field.string = { $field } స్ట్రింగ్ అయి ఉండాలి

journal.cli.share.transport.field.strings = { $field } లో కేవలం స్ట్రింగ్‌లు ఉండాలి

journal.cli.share.transport.object = నమోదు { $json } ఆబ్జెక్ట్ అయి ఉండాలి

journal.cli.share.transport.outcome = రెస్పాన్స్ ఫలితానికి మద్దతు లేదు

journal.cli.share.transport.pipe.missing = వైర్ { $pipe } అందుబాటులో లేదు

journal.cli.share.transport.request.empty = { $contents } ఖాళీ కాకూడదు

journal.cli.share.transport.request.overflow = { $overflow } కు మొదటి కంటెంట్ భాగం వచనం అయి ఉండాలి

journal.cli.share.transport.schema = నమోదు స్కీమాకు మద్దతు లేదు

journal.cli.share.transport.stdout.limit = { $stdout } { $limit } బైట్‌ల వద్ద కత్తిరించబడింది

journal.cli.share.transport.usage = { $usage } తప్పనిసరి

journal.cli.share.transport.worker.channel = పైప్ వర్కర్ పూర్తి ఛానెల్: { $detail }

journal.cli.share.transport.worker.join = { $pipe } వర్కర్ జాయిన్: థ్రెడ్ పానిక్ అయింది

journal.cli.share.transport.worker.start = { $pipe } వర్కర్ స్టార్ట్: { $detail }

journal.cli.share.transport.writer = { $stdin } రైటర్: { $detail }

journal.cli.share.transport.writer.panic = { $stdin } రైటర్: థ్రెడ్ పానిక్ అయింది

journal.cli.share.usage.match = usage: { $syntax }

journal.cli.share.usage.package = usage: { $syntax }

journal.cli.share.usage.root = usage: { $syntax }

journal.cli.social.failed = పూర్తి చేయలేకపోయింది: { $detail }

journal.cli.social.follow.done = { $sharer } మీతో పంచుకున్న space ను ఫాలో చేస్తున్నారు.

journal.cli.social.receive.added = { $activity_quantity ->
    [one] { $sharer } మీతో { $activities } కొత్త కార్యకలాపం పంచుకున్నారు.
   *[other] { $sharer } మీతో { $activities } కొత్త కార్యకలాపాలు పంచుకున్నారు.
}

journal.cli.social.receive.failed = { $sharer } మీతో పంచుకున్న space చదవలేకపోయింది: { $detail }

journal.cli.social.receive.none = { $sharer } నుండి కొత్తది ఏమీ లేదు.

journal.cli.social.receive.nothing = మీరు ఇంకా ఏ space నూ ఫాలో చేయడం లేదు.

journal.cli.social.receive.removed = { $activity_quantity ->
    [one] { $sharer } { $activities } కార్యకలాపం పంచుకోవడం ఆపారు, కాబట్టి అది మీ జర్నల్ నుండి పోయింది.
   *[other] { $sharer } { $activities } కార్యకలాపాలు పంచుకోవడం ఆపారు, కాబట్టి అవి మీ జర్నల్ నుండి పోయాయి.
}

journal.cli.social.receive.updated = { $activity_quantity ->
    [one] { $sharer } మీతో పంచుకున్న { $activities } కార్యకలాపాన్ని అప్‌డేట్ చేశారు.
   *[other] { $sharer } మీతో పంచుకున్న { $activities } కార్యకలాపాలను అప్‌డేట్ చేశారు.
}

journal.cli.social.resolve.done = { $account } అంటే { $did }, { $host } వద్ద హోస్ట్ అయి ఉంది.

journal.cli.social.share.done = "{ $title }" పంచుకున్నారు. మీరు దీన్ని పంచుకున్న వ్యక్తులకు ఈ space చిరునామా పంపండి, వారి జర్నల్‌లు దీన్ని ఫాలో చేయగలిగేలా:{ $line_break }{ $space }

journal.cli.social.signed.out = మీరు సైన్ ఇన్ అయి లేరు. { $sign_in } తో మొదలుపెట్టండి

journal.cli.social.signin.declined = సైన్-ఇన్ నిరాకరించబడింది, కాబట్టి ఏమీ మారలేదు.

journal.cli.social.signin.done = { $account } గా సైన్ ఇన్ అయి ఉన్నారు.

journal.cli.social.signin.expired = సైన్-ఇన్ సమయానికి పూర్తి కాలేదు. మొదటి నుండి మొదలుపెట్టడానికి దాన్ని మళ్ళీ రన్ చేయండి.

journal.cli.social.signin.open = సైన్ ఇన్ చేయడానికి, ఈ చిరునామాను మీ బ్రౌజర్‌లో తెరవండి:{ $line_break }{ $address }

journal.cli.social.status.account = { $account } గా { $host } లో సైన్ ఇన్ అయి ఉన్నారు.

journal.cli.social.status.memories = { $memory_quantity ->
    [one] మీతో పంచుకున్న { $memories } కార్యకలాపం ఉంచుతోంది.
   *[other] మీతో పంచుకున్న { $memories } కార్యకలాపాలు ఉంచుతోంది.
}

journal.cli.social.status.shares = { $share_quantity ->
    [one] { $shares } కార్యకలాపం పంచుకుంటోంది.
   *[other] { $shares } కార్యకలాపాలు పంచుకుంటోంది.
}

journal.cli.social.status.spaces = { $space_quantity ->
    [one] { $spaces } space ఫాలో చేస్తోంది.
   *[other] { $spaces } spaces ఫాలో చేస్తోంది.
}

journal.cli.social.sync.current = పంచుకున్న ప్రతి కార్యకలాపం తాజాగా ఉంది.

journal.cli.social.sync.removed = { $activity_quantity ->
    [one] మీ జర్నల్‌లో ఇక లేని { $activities } పంచుకున్న కార్యకలాపం తీసివేయబడింది.
   *[other] మీ జర్నల్‌లో ఇక లేని { $activities } పంచుకున్న కార్యకలాపాలు తీసివేయబడ్డాయి.
}

journal.cli.social.sync.updated = { $activity_quantity ->
    [one] { $activities } పంచుకున్న కార్యకలాపం అప్‌డేట్ అయింది.
   *[other] { $activities } పంచుకున్న కార్యకలాపాలు అప్‌డేట్ అయ్యాయి.
}

journal.cli.social.unshare.done = ఆ కార్యకలాపం పంచుకోవడం ఆగింది, మరియు మీరు దాన్ని పంచుకున్న ప్రతి space నుండి అది పోయింది.

journal.cli.social.unshare.unknown = ఆ కార్యకలాపం పంచుకోబడలేదు.

journal.cli.social.usage = usage: { $syntax }{ $line_break }{ $line_break }మీ జర్నల్ నుండి ఒక కార్యకలాపాన్ని మీరు ఎంచుకున్న వ్యక్తులతో, మీ సొంత atproto ఖాతాలోని space ద్వారా పంచుకోండి, మరియు ఇతరులు మీతో పంచుకునే spaces ను ఫాలో చేయండి. మీ జర్నల్ వారు పంచుకున్నదాన్ని మీ సొంత జ్ఞాపకాల నుండి వేరుగా ఉంచుతుంది. కేవలం డెవలప్‌మెంట్ బిల్డ్‌లు.

journal.cli.speakers.backfill.error.flags.mismatch = ఆపరేషన్ { $operation_id } కోసం మార్పులేని బ్యాక్‌ఫిల్ ఫ్లాగ్‌లు సరిపోలేదు: ఉండాల్సింది commit={ $expected_commit } reattribute={ $expected_reattribute } accumulation={ $expected_accumulation }, వచ్చింది commit={ $got_commit } reattribute={ $got_reattribute } accumulation={ $got_accumulation }

journal.cli.speakers.backfill.error.invalid.flags = చెల్లని బ్యాక్‌ఫిల్ ఫ్లాగ్‌లు: { $detail }

journal.cli.speakers.backfill.error.journal.root = జర్నల్ రూట్ అంగీకరించలేకపోయింది: { $error }

journal.cli.speakers.backfill.error.ledger = లెడ్జర్ ఆపరేషన్ విఫలమైంది: { $error }

journal.cli.speakers.backfill.error.lock = బ్యాక్‌ఫిల్ అమలు లాక్ చేయలేకపోయింది: { $error }

journal.cli.speakers.backfill.error.not.found = బ్యాక్‌ఫిల్ ఆపరేషన్ కనబడలేదు: { $id }

journal.cli.speakers.backfill.error.scan = బ్యాక్‌ఫిల్ స్కాన్ చేయలేకపోయింది: { $error }

journal.cli.speakers.owner.voice.error.identity.invalid = కాన్ఫిగ్ చేసిన మీ గుర్తింపు అంగీకరించబడలేదు

journal.cli.speakers.owner.voice.error.no.candidate = అభ్యర్థి అందుబాటులో లేదు

journal.cli.speakers.owner.voice.error.no.saved.voice = మీ గొంతు సేవ్ అయి లేదు

journal.cli.speakers.owner.voice.error.stale.candidate = అభ్యర్థి వెర్షన్ సరిపోలేదు లేదా జనరేషన్ పాతబడింది

journal.cli.speakers.pool.error.centroid.width = అభ్యర్థి పూల్ సెంట్రాయిడ్ వెడల్పు సరిపోలేదు

journal.cli.speakers.pool.error.lock = అభ్యర్థి పూల్ లాక్ చేయలేకపోయింది: { $error }

journal.cli.speakers.pool.error.read = అభ్యర్థి పూల్ చదవలేకపోయింది: { $error }

journal.cli.speakers.pool.error.write = అభ్యర్థి పూల్ రాయలేకపోయింది: { $error }

journal.cli.speakers.pool.refusal.days.unreadable = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: జర్నల్ రోజులు జాబితా చేయలేకపోయాయి

journal.cli.speakers.pool.refusal.format.unsupported = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: గొంతు జాబితా ఈ వెర్షన్ చేర్చలేని ఆకృతిలో ఉంది

journal.cli.speakers.pool.refusal.lock.timeout = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: గొంతు జాబితా లాక్ టైమ్‌అవుట్ అయింది

journal.cli.speakers.pool.refusal.lock.timeout.kept = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: గొంతు జాబితా లాక్ టైమ్‌అవుట్ అయింది; చేర్చినది ఉంచబడింది

journal.cli.speakers.pool.refusal.pool.unreadable = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: గొంతు జాబితా చదవలేకపోయింది

journal.cli.speakers.pool.refusal.pool.write.failed = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: గొంతు జాబితా రాయలేకపోయింది

journal.cli.speakers.pool.refusal.pool.write.failed.kept = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: గొంతు జాబితా రాయలేకపోయింది; చేర్చినది ఉంచబడింది

journal.cli.speakers.pool.refusal.progress.invalid = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: క్యాచ్-అప్ ప్రోగ్రెస్ ఫైల్ చెల్లదు; మీ జర్నల్ నుండి awareness/speaker_candidates_backfill.json తీసివేస్తే క్యాచ్-అప్ సురక్షితంగా రీస్టార్ట్ అవుతుంది

journal.cli.speakers.pool.refusal.progress.unreadable = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: క్యాచ్-అప్ ప్రోగ్రెస్ ఫైల్ చదవలేకపోయింది; మీ జర్నల్‌లోని awareness ఫోల్డర్ చదవగలదో సరిచూడండి

journal.cli.speakers.pool.refusal.progress.write.failed = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: క్యాచ్-అప్ ప్రోగ్రెస్ ఫైల్ రాయలేకపోయింది

journal.cli.speakers.pool.refusal.progress.write.failed.kept = గొంతు జాబితా క్యాచ్-అప్ నిరాకరించబడింది: క్యాచ్-అప్ ప్రోగ్రెస్ ఫైల్ రాయలేకపోయింది; చేర్చినది ఉంచబడింది

journal.cli.stats.arguments = usage: { $syntax }{ $line_break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $argument }

journal.cli.stats.cause.facet = ఫాసెట్ డేటా నిల్వ లోపం: { $detail }

journal.cli.stats.cause.invalid.day = చెల్లని రోజు { $day }; { $date_format } క్యాలెండర్ తేదీ అవసరం

journal.cli.stats.cause.io = { $path } యాక్సెస్ చేయలేకపోయింది: { $detail }

journal.cli.stats.cause.path = జర్నల్ పాత్ లోపం: { $detail }

journal.cli.stats.cause.read = జర్నల్ చదవడం లోపం: { $detail }

journal.cli.stats.cause.schema = గణాంకాల సరిచూపు విఫలమైంది: { $schema } { $actual } గా ఉంది, { $expected } అవసరం

journal.cli.stats.cause.timestamp = గణాంకాల సరిచూపు విఫలమైంది: { $timestamp } ఖాళీ కాకూడదు

journal.cli.stats.cause.validation = గణాంకాల సరిచూపు విఫలమైంది: { $detail }

journal.cli.stats.cause.write = జర్నల్ అటామిక్ రాత లోపం: { $detail }

journal.cli.stats.configuration = జర్నల్ రోజువారీ కాన్ఫిగ్ లోడ్ చేయలేకపోయింది: { $error }

journal.cli.stats.day.cache = { $day } కోసం రోజు కాష్ సేవ్ చేయలేకపోయింది: { $error }

journal.cli.stats.enumerate = జర్నల్ రోజులు లెక్కించలేకపోయింది: { $error }

journal.cli.stats.help = usage: { $syntax }{ $line_break }{ $line_break }{ $brand } జర్నల్‌ను స్కాన్ చేసి గణాంకాలు రూపొందించండి{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_5 }ఈ సహాయ సందేశం చూపించి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $no_cache_option }{ $gap_5 }రోజువారీ కాష్ డిసేబుల్ చేయండి (అన్ని రోజులను బలవంతంగా మళ్ళీ స్కాన్ చేయండి){ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_2 }వివరమైన అవుట్‌పుట్ ఎనేబుల్ చేయండి{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_4 }డీబగ్ లాగ్ ఎనేబుల్ చేయండి

journal.cli.stats.overrides = గణాంకాల సరిచూపు విఫలమైంది: { $path } నుండి టాలెంట్ ఓవర్‌రైడ్‌లు చదవలేకపోయింది

journal.cli.stats.publication.durable = ప్రచురించబడింది; నిలకడ ఖచ్చితం కాదు: { $source }

journal.cli.stats.publication.observed = ప్రచురించబడింది; పరిశీలన ఖచ్చితం కాదు: { $source }

journal.cli.stats.roots = journal-stats విఫలమైంది: { $error }

journal.cli.stats.saved = గణాంకాలు { $path } కు సేవ్ అయ్యాయి

journal.cli.stats.tokens.directory = టోకెన్ డైరెక్టరీ చదవలేకపోయింది: { $path }

journal.cli.stats.tokens.json = చెల్లని టోకెన్ { $json_format } { $path } లో: { $error }

journal.cli.stats.tokens.load = { $path } కోసం టోకెన్ కాష్ లోడ్ చేయలేకపోయింది: { $error }

journal.cli.stats.tokens.read = { $path } కోసం టోకెన్ చదవలేకపోయింది: { $error }

journal.cli.stats.tokens.save = { $path } కోసం టోకెన్ కాష్ సేవ్ చేయలేకపోయింది: { $error }

journal.cli.stats.validate = { $filename } సరిచూడలేకపోయింది: { $error }

journal.cli.stats.write = { $filename } రాయలేకపోయింది: { $error }

journal.cli.stats.written = { $days ->
    [one] { $days } రోజు గణాంకాలు { $path } కు రాయబడ్డాయి
   *[other] { $days } రోజుల గణాంకాలు { $path } కు రాయబడ్డాయి
}

journal.cli.streams.arguments = usage: { $syntax }{ $break }{ $command }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.streams.empty = ప్రవాహాలు ఏవీ కనబడలేదు

journal.cli.streams.header.day = చివరి రోజు

journal.cli.streams.header.name = పేరు

journal.cli.streams.header.segment = చివరి సెగ్మెంట్

journal.cli.streams.header.sequence = క్రమం

journal.cli.streams.header.type = రకం

journal.cli.streams.help = usage: { $syntax }{ $paragraph }ప్రవాహ గుర్తింపును పరిశీలించి నిర్వహించండి{ $paragraph }స్థాన ఆర్గ్యుమెంట్‌లు:{ $break }{ $indent }{ $name_input }{ $name_space }పరిశీలించాల్సిన ప్రవాహ పేరు (మినహాయిస్తే అన్ని ప్రవాహాలు జాబితా అవుతాయి){ $paragraph }ఆప్షన్‌లు:{ $break }{ $indent }{ $help_input }{ $help_space }ఈ సహాయ సందేశం చూపించి నిష్క్రమించండి{ $break }{ $indent }{ $rebuild_input }{ $rebuild_space }ప్రతి సెగ్మెంట్ గుర్తుల నుండి ప్రవాహ స్థితిని మళ్ళీ బిల్డ్ చేయండి{ $break }{ $indent }{ $verbose_input }{ $verbose_space }వివరమైన అవుట్‌పుట్ ఎనేబుల్ చేయండి{ $break }{ $indent }{ $debug_input }{ $debug_space }డీబగ్ లాగ్ ఎనేబుల్ చేయండి

journal.cli.streams.inspect.malformed = ప్రవాహం { $path } చదవలేకపోయింది: ఆకృతి తప్పు నమోదు

journal.cli.streams.inspect.not.found = ప్రవాహం కనబడలేదు: { $name }

journal.cli.streams.list.error = ప్రవాహాలు జాబితా చేయలేకపోయింది: { $error }

journal.cli.streams.marker.sequence = చదవలేని గుర్తు { $path }: చెల్లని క్రమం

journal.cli.streams.marker.unreadable = చదవలేని గుర్తు { $path }: గుర్తు చదవలేకపోయింది

journal.cli.streams.rebuild.empty = { $segment_quantity ->
    [one] ప్రవాహాలు ఏవీ కనబడలేదు ({ $segments } సెగ్మెంట్ స్కాన్ అయింది)
   *[other] ప్రవాహాలు ఏవీ కనబడలేదు ({ $segments } సెగ్మెంట్‌లు స్కాన్ అయ్యాయి)
}

journal.cli.streams.rebuild.result = { $stream_quantity ->
    [one] { $streams } ప్రవాహం మళ్ళీ బిల్డ్ అయింది, { $segment_quantity ->
        [one] { $segments } సెగ్మెంట్ నుండి:
       *[other] { $segments } సెగ్మెంట్‌ల నుండి:
    }
   *[other] { $streams } ప్రవాహాలు మళ్ళీ బిల్డ్ అయ్యాయి, { $segment_quantity ->
        [one] { $segments } సెగ్మెంట్ నుండి:
       *[other] { $segments } సెగ్మెంట్‌ల నుండి:
    }
}

journal.cli.streams.record.malformed = ప్రవాహ నమోదు { $path } చదవలేకపోయింది: ఆకృతి తప్పు నమోదు

journal.cli.streams.repair.ahead = { $stream }: ప్రవాహ స్థితి క్రమం గుర్తుల కంటే ముందు ఉంది; మారలేదు

journal.cli.streams.repair.locked = { $stream }: { $path } లాక్ చేయలేకపోయింది; మారలేదు

journal.cli.streams.repair.malformed = { $stream }: { $path } వద్ద ఆకృతి తప్పు ప్రవాహ నమోదు; మారలేదు

journal.cli.streams.repair.missing = { $stream }: { $path } వద్ద ప్రవాహ నమోదు లేదు; మారలేదు

journal.cli.streams.repair.write.failed = { $stream }: { $path } రాయలేకపోయింది: అటామిక్ ప్రచురణ విఫలమైంది

journal.cli.streams.segments.error = సెగ్మెంట్‌లు జాబితా చేయలేకపోయింది: { $error }

journal.cli.streams.supervisor = జర్నల్ నడవడం లేదు. '{ $command }' తో స్టార్ట్ చేసి మళ్ళీ ప్రయత్నించండి.

journal.cli.talent.compose.facet.missing = ఫాసెట్ '{ $facet }' కనబడలేదు

journal.cli.talent.compose.prompt.path.has.no.parent = టాలెంట్ { $talent }: ప్రాంప్ట్ పాత్‌కు పేరెంట్ లేదు

journal.cli.talent.compose.prompt.path.is.missing = టాలెంట్ { $talent }: ప్రాంప్ట్ పాత్ లేదు

journal.cli.talent.compose.schema.directory.is.unavailable = టాలెంట్ { $talent }: స్కీమా డైరెక్టరీ అందుబాటులో లేదు: { $error }

journal.cli.talent.compose.schema.file.could.not.be.read = టాలెంట్ { $talent }: స్కీమా ఫైల్ చదవలేకపోయింది: { $error }

journal.cli.talent.compose.schema.file.is.not.a.valid.json.schema = టాలెంట్ { $talent }: స్కీమా ఫైల్ చెల్లుబాటు అయ్యే { $schema_format } కాదు: { $path }

journal.cli.talent.compose.schema.file.is.not.valid.json = టాలెంట్ { $talent }: స్కీమా ఫైల్ చెల్లుబాటు అయ్యే { $json } కాదు: { $path }

journal.cli.talent.compose.schema.file.not.found.cause = టాలెంట్ { $talent }: స్కీమా ఫైల్ కనబడలేదు: { $error }

journal.cli.talent.compose.schema.file.not.found.path = టాలెంట్ { $talent }: స్కీమా ఫైల్ కనబడలేదు: { $path }

journal.cli.talent.compose.schema.must.be.a.string.got = టాలెంట్ { $talent }: స్కీమా స్ట్రింగ్ అయి ఉండాలి, వచ్చింది { $type_name }: { $value }

journal.cli.talent.compose.schema.path.escapes.talent.directory = టాలెంట్ { $talent }: స్కీమా పాత్ టాలెంట్ డైరెక్టరీని దాటుతుంది: { $path }

journal.cli.talent.compose.schema.path.must.be.relative = టాలెంట్ { $talent }: స్కీమా పాత్ సాపేక్షంగా ఉండాలి: { $path }

journal.cli.talent.compose.schema.path.must.not.contain = టాలెంట్ { $talent }: స్కీమా పాత్ '..' కలిగి ఉండకూడదు: { $path }

journal.cli.talent.compose.settings = జర్నల్ కాన్ఫిగ్ చదవలేకపోయింది: { $detail }

journal.cli.talent.config.access.retired = ప్రాంప్ట్ '{ $talent }' వాడుకలో లేని { $access_tier } పెడుతుంది

journal.cli.talent.config.activities = కార్యకలాప-షెడ్యూల్ ప్రాంప్ట్ '{ $talent }' కు ఖాళీ కాని { $activities } జాబితా ఉండాలి (సరిపోయే కార్యకలాప రకాలు, లేదా అన్ని రకాలకు { $all_types }).

journal.cli.talent.config.budget.invalid = ప్రాంప్ట్ '{ $talent }' లో చెల్లని { $max_output_tokens } { $value } ఉంది. బండిల్ మోడల్‌తో కొలిచిన ధనాత్మక పూర్ణాంకం అవసరం.

journal.cli.talent.config.budget.required = ప్రాంప్ట్ '{ $talent }' లో తప్పనిసరి { $max_output_tokens } లేదు. బండిల్ మోడల్‌పై దాని అతిపెద్ద అవుట్‌పుట్‌ను కొలవండి, దాని 1.5 రెట్లను 256 గుణకానికి పైకి సవరించి ప్రకటించండి.

journal.cli.talent.config.budget.retired = ప్రాంప్ట్ '{ $talent }' వాడుకలో లేని { $thinking_budget } ప్రకటిస్తుంది. ఆలోచన కేవలం ఆలోచనలో, మీ సొంత మోడల్ కోసం పెట్టబడుతుంది.

journal.cli.talent.config.cwd.retired = ప్రాంప్ట్ '{ $talent }' వాడుకలో లేని { $cwd } పెడుతుంది

journal.cli.talent.config.frontmatter = { $path } నుండి ఫ్రంట్‌మాటర్ పార్స్ చేయలేకపోయింది

journal.cli.talent.config.output.required = ప్రాంప్ట్ '{ $talent }' లో type={ $generate } ఉంది, కానీ తప్పనిసరి { $output } ఫీల్డ్ లేదు.

journal.cli.talent.config.priority = షెడ్యూల్ ప్రాంప్ట్ '{ $talent }' లో తప్పనిసరి { $priority } ఫీల్డ్ లేదు. { $schedule } ఉన్న అన్ని ప్రాంప్ట్‌లు స్పష్టమైన ప్రాధాన్యత ప్రకటించాలి.

journal.cli.talent.config.read = { $path } చదవలేకపోయింది: { $error }

journal.cli.talent.config.stat = { $path } stat విఫలమైంది: { $error }

journal.cli.talent.config.stem = టాలెంట్ ఫైల్ పేరుకు { $encoding } స్టెమ్ లేదు: { $path }

journal.cli.talent.config.type.invalid = ప్రాంప్ట్ '{ $talent }' లో చెల్లని రకం { $value } ఉంది. { $generate } అవసరం.

journal.cli.talent.config.type.required = ప్రాంప్ట్ '{ $talent }' లో అవుట్‌పుట్ ఉంది, కానీ తప్పనిసరి { $type } ఫీల్డ్ లేదు.

journal.cli.talent.daily.accepted.response = అంగీకరించిన రెస్పాన్స్ లేదు

journal.cli.talent.daily.accepted.result = అంగీకరించిన రోజువారీ ఫలితం లేదు

journal.cli.talent.daily.artifact.before = వైరుధ్యం: ప్రాంప్ట్ తయారీ తర్వాత తప్పనిసరి ఆర్టిఫ్యాక్ట్ మారింది

journal.cli.talent.daily.artifact.changed = వైరుధ్యం: తయారీ తర్వాత తప్పనిసరి అవుట్‌పుట్ మారింది

journal.cli.talent.daily.artifact.snapshot = ఫ్రోజెన్ తప్పనిసరి ఆర్టిఫ్యాక్ట్ స్నాప్‌షాట్ లేదు

journal.cli.talent.daily.artifact.snapshot.invalid = చెల్లని ఫ్రోజెన్ ఆర్టిఫ్యాక్ట్ స్నాప్‌షాట్: { $error }

journal.cli.talent.daily.attempt.day = రోజువారీ ప్రయత్నం రోజు లేదు

journal.cli.talent.daily.attempt.day.invalid = చెల్లని రోజువారీ ప్రయత్నం రోజు

journal.cli.talent.daily.attempt.mismatch = రోజువారీ { $key } సరిపోలేదు

journal.cli.talent.daily.attempt.token = రోజువారీ ప్రయత్నం టోకెన్ లేదు

journal.cli.talent.daily.commit.observer = ఎంటిటీ వాస్తవాల స్థితి అవసరం

journal.cli.talent.daily.commit.suggest = ఎంటిటీ సూచన స్థితి అవసరం

journal.cli.talent.daily.commit.text = వచన అవుట్‌పుట్ అవసరం

journal.cli.talent.daily.facet.changed = వైరుధ్యం: ప్రాంప్ట్ తయారీ తర్వాత కలిగి ఉన్న ఫాసెట్ మారింది

journal.cli.talent.daily.facet.identity = ఫ్రోజెన్ ఫాసెట్ గుర్తింపు లేదు

journal.cli.talent.daily.facet.path = చెల్లని ఫాసెట్ అవుట్‌పుట్ పాత్

journal.cli.talent.daily.maintenance.changed = వైరుధ్యం: ప్రాంప్ట్ తయారీ తర్వాత రోజువారీ సమయం మారింది

journal.cli.talent.daily.maintenance.output = చెల్లని మెయింటెనెన్స్ అవుట్‌పుట్: { $error }

journal.cli.talent.daily.maintenance.primary = మెయింటెనెన్స్ అవుట్‌పుట్‌లో ప్రైమరీ లేదు

journal.cli.talent.daily.maintenance.snapshot = ఫ్రోజెన్ రోజువారీ సమయం స్నాప్‌షాట్ లేదు

journal.cli.talent.daily.observer.changed = వైరుధ్యం: ప్రాంప్ట్ తయారీ తర్వాత పరిశీలన మారింది

journal.cli.talent.daily.observer.conflict = వైరుధ్యం: { $error }

journal.cli.talent.daily.observer.entity.gone = వైరుధ్యం: పరిశీలన ఎంటిటీ కనబడకుండా పోయింది

journal.cli.talent.daily.observer.validation = సరిచూపు: { $error }

journal.cli.talent.daily.output.outside = రోజువారీ అవుట్‌పుట్ జర్నల్ బయట ఉంది

journal.cli.talent.daily.output.path = చెల్లని రోజువారీ అవుట్‌పుట్ పాత్

journal.cli.talent.daily.owner.alias.claimed = వైరుధ్యం: తయారీ తర్వాత ప్రమోషన్ అలియాస్ క్లెయిమ్ చేయబడింది

journal.cli.talent.daily.owner.alias.target.detached = వైరుధ్యం: అలియాస్ టార్గెట్ ఇక జోడించి లేదు

journal.cli.talent.daily.owner.identity.blocked.facet = వైరుధ్యం: ఈ ఫాసెట్‌లో ప్రమోషన్ పేరు దాచబడింది

journal.cli.talent.daily.owner.identity.blocked.hidden = వైరుధ్యం: ఎంటిటీ దాచబడి ఉంది, కాబట్టి దాని పేర్లు మారలేదు

journal.cli.talent.daily.owner.identity.blocked.identity = వైరుధ్యం: ప్రమోషన్ గుర్తింపు దాచబడింది

journal.cli.talent.daily.owner.identity.changed = వైరుధ్యం: తయారీ తర్వాత ప్రమోట్ అయిన గుర్తింపు మారింది

journal.cli.talent.daily.owner.identity.deleted = వైరుధ్యం: తయారీ తర్వాత ప్రమోట్ అయిన గుర్తింపు తొలగించబడింది

journal.cli.talent.daily.owner.identity.disappeared = వైరుధ్యం: ప్రమోషన్ గుర్తింపు కనబడకుండా పోయింది

journal.cli.talent.daily.owner.identity.map.group.lost = వైరుధ్యం: ప్రమోషన్ { $name } { $directory } తో సరిపోతుంది, అది తన గుర్తింపు మ్యాప్ సమూహాన్ని కోల్పోయింది

journal.cli.talent.daily.owner.identity.merged = వైరుధ్యం: తయారీ తర్వాత ప్రమోట్ అయిన గుర్తింపు జత చేసి తీసివేయబడింది

journal.cli.talent.daily.owner.identity.moved = వైరుధ్యం: తయారీ తర్వాత ప్రమోట్ అయిన గుర్తింపు తరలింది

journal.cli.talent.daily.owner.identity.unreadable = వైరుధ్యం: ప్రమోషన్ { $name } ఎంటిటీ ఐడి { $slug } ను ఉత్పన్నం చేస్తుంది, దాని గుర్తింపు { $directory } చదవలేం

journal.cli.talent.daily.owner.merge.proposals.changed = వైరుధ్యం: తయారీ తర్వాత జత ప్రతిపాదనలు మారాయి

journal.cli.talent.daily.owner.name.matches.multiple = వైరుధ్యం: ప్రమోషన్ { $name } పేరు ప్రకారం { $count } గుర్తింపులతో సరిపోతుంది

journal.cli.talent.daily.owner.no.usable.entity.id = వైరుధ్యం: ప్రమోషన్ { $name } కు వాడగల ఎంటిటీ ఐడి లేదు

journal.cli.talent.daily.owner.owning.facet.changed.absent = వైరుధ్యం: కలిగి ఉన్న ఫాసెట్ ఇక లేదు

journal.cli.talent.daily.owner.owning.facet.changed.invalid = వైరుధ్యం: కలిగి ఉన్న ఫాసెట్‌కు చెల్లుబాటు అయ్యే స్థిర గుర్తింపు లేదు

journal.cli.talent.daily.owner.owning.facet.changed.replaced = వైరుధ్యం: తయారీ తర్వాత కలిగి ఉన్న ఫాసెట్ భర్తీ అయింది

journal.cli.talent.daily.owner.principal.grant.refused.exists = వైరుధ్యం: జర్నల్‌కు ఇప్పటికే మీరు ఉన్నారు, కాబట్టి ఈ వ్యక్తి గుర్తు పెట్టబడలేదు

journal.cli.talent.daily.owner.principal.grant.refused.name = వైరుధ్యం: కాన్ఫిగ్ చేసిన మీ పేరు ఇక ఈ వ్యక్తితో సరిపోదు

journal.cli.talent.daily.owner.principal.settings = జర్నల్ సెట్టింగ్‌లు చదవలేకపోయాయి, కాబట్టి ఈ వ్యక్తి మీరుగా గుర్తు పెట్టబడలేదు

journal.cli.talent.daily.owner.promoted.entity.blocked.hidden = వైరుధ్యం: ప్రమోట్ అయిన ఎంటిటీ దాచబడి ఉంది

journal.cli.talent.daily.owner.promoted.entity.blocked.name = వైరుధ్యం: ప్రమోట్ అయిన ఎంటిటీ { $name } దాచబడి ఉంది

journal.cli.talent.daily.owner.relationship.changed = వైరుధ్యం: తయారీ తర్వాత ప్రమోషన్ సంబంధం మారింది

journal.cli.talent.daily.owner.relationship.occupied = వైరుధ్యం: ప్రమోషన్ { $name } కు సంబంధ డైరెక్టరీ { $relationship_dir } అవసరం, అది ఇప్పటికే ఆక్రమించబడి ఉంది

journal.cli.talent.daily.packet.absent = రోజువారీ ప్రయత్నానికి ఫ్రోజెన్ ప్యాకెట్ లేదు

journal.cli.talent.daily.packet.digest = రోజువారీ ప్యాకెట్ digest సరిపోలేదు

journal.cli.talent.daily.packet.scope = రోజువారీ ప్యాకెట్ స్కోప్ సరిపోలేదు

journal.cli.talent.daily.packet.talent = రోజువారీ ప్యాకెట్ టాలెంట్ సరిపోలేదు

journal.cli.talent.daily.parse.entities = ఎంటిటీలు అర్రే అయి ఉండాలి

journal.cli.talent.daily.parse.output = అవుట్‌పుట్‌ను JSON గా పార్స్ చేయలేకపోయింది: { $error }

journal.cli.talent.daily.publication.plan = ప్రచురణకు నిలుపుకున్న జనరేట్ అయిన ఫలితం మరియు సరిగ్గా తయారు చేసిన ప్లాన్ అవసరం

journal.cli.talent.daily.publication.record = ప్రచురణ నమోదు లేదు

journal.cli.talent.daily.response.malformed = నిలుపుకున్న రోజువారీ రెస్పాన్స్ ఆకృతి తప్పు

journal.cli.talent.daily.retry.scope = రోజువారీ మళ్ళీ ప్రయత్నం స్కోప్ మారింది

journal.cli.talent.daily.review.aliases = సమీక్ష ప్రమోషన్‌కు అలియాస్‌లు లేవు

journal.cli.talent.daily.review.eligibility = ఫ్రోజెన్ సమీక్ష అర్హత లేదు

journal.cli.talent.daily.review.hints = ఫ్రోజెన్ సమీక్ష హింట్‌లు లేవు

journal.cli.talent.daily.review.inputs = ఫ్రోజెన్ సమీక్ష అంగీకార ఇన్‌పుట్‌లు లేవు

journal.cli.talent.daily.review.merges = సమీక్ష అవుట్‌పుట్‌లో జతలు లేవు

journal.cli.talent.daily.review.name = ఫ్రోజెన్ అభ్యర్థికి పేరు లేదు

journal.cli.talent.daily.review.owner = ఫ్రోజెన్ సమీక్షలో మీ స్థితి లేదు

journal.cli.talent.daily.review.promotion = ఫ్రోజెన్ ప్రమోషన్‌లో మీ స్థితి లేదు

journal.cli.talent.daily.review.promotion.changed = వైరుధ్యం: ప్రాంప్ట్ తయారీ తర్వాత ప్రమోషన్‌లో మీ స్థితి మారింది

journal.cli.talent.daily.review.promotions = సమీక్ష అవుట్‌పుట్‌లో ప్రమోషన్‌లు లేవు

journal.cli.talent.daily.review.proposals = ఫ్రోజెన్ ప్రతిపాదన స్థితి లేదు

journal.cli.talent.daily.review.proposals.changed = సంఘర్షణ: ప్రాంప్ట్ సిద్ధం చేసిన తర్వాత జత ప్రతిపాదన మారింది

journal.cli.talent.daily.review.type = ఫ్రోజెన్ అభ్యర్థికి రకం లేదు

journal.cli.talent.daily.type = మద్దతు లేని టాలెంట్ రకం: { $kind }

journal.cli.talent.daily.validation.aliases = సరిచూపు: ప్రమోట్ చేయబడిన సమీక్ష అభ్యర్థిలో అలియాస్‌ల అర్రే ఉండాలి

journal.cli.talent.daily.validation.json = సరిచూపు: చెల్లని { $shape } JSON: { $error }

journal.cli.talent.daily.validation.maintenance.missing = సరిచూపు: మెయింటెనెన్స్ ప్రైమరీ లేదు

journal.cli.talent.daily.validation.maintenance.time = సరిచూపు: మెయింటెనెన్స్ ప్రైమరీ HH:MM అయి ఉండాలి

journal.cli.talent.daily.validation.merges = సరిచూపు: సమీక్ష జతలు అర్రే అయి ఉండాలి

journal.cli.talent.daily.validation.observer = సరిచూపు: entities:entity_observer కు ఎంటిటీల అర్రే అవసరం

journal.cli.talent.daily.validation.promotions = సరిచూపు: సమీక్ష ప్రమోషన్‌లు అర్రే అయి ఉండాలి

journal.cli.talent.daily.validation.suggest = సరిచూపు: సూచన ఎంటిటీలు అర్రే అయి ఉండాలి

journal.cli.talent.daily.write.contract = { $talent } కోసం మద్దతు లేని రోజువారీ రాత ఒప్పందం

journal.cli.talent.error.list.choice = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్ { $flag }: చెల్లని ఎంపిక: '{ $value }' ({ $choices } నుండి ఎంచుకోండి)

journal.cli.talent.error.list.unknown = usage: { $syntax }{ $line_break }{ $program }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.talent.error.list.value = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్ { $flag }: ఒక ఆర్గ్యుమెంట్ అవసరం

journal.cli.talent.error.log.required = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $arguments }

journal.cli.talent.error.logs.count = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్ { $count_options }: చెల్లని పూర్ణాంకం విలువ: '{ $value }'

journal.cli.talent.error.logs.value = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్ { $flag }: ఒక ఆర్గ్యుమెంట్ అవసరం

journal.cli.talent.error.passages.required = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $arguments }

journal.cli.talent.error.passages.unknown = usage: { $syntax }{ $line_break }{ $program }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.talent.error.passages.value = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్ { $flag }: ఒక ఆర్గ్యుమెంట్ అవసరం

journal.cli.talent.error.root.choice = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్ { $subcommand }: చెల్లని ఎంపిక: '{ $value }' ({ $choices } నుండి ఎంచుకోండి)

journal.cli.talent.error.root.encoding = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్‌లు చెల్లుబాటు అయ్యే { $encoding } కావు

journal.cli.talent.error.root.unknown = usage: { $syntax }{ $line_break }{ $program }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.talent.error.show.required = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఈ ఆర్గ్యుమెంట్‌లు తప్పనిసరి: { $arguments }

journal.cli.talent.error.show.unknown = usage: { $syntax }{ $line_break }{ $program }: లోపం: గుర్తించని ఆర్గ్యుమెంట్‌లు: { $arguments }

journal.cli.talent.error.show.value = usage: { $syntax }{ $line_break }{ $program }: లోపం: ఆర్గ్యుమెంట్ { $flag }: ఒక ఆర్గ్యుమెంట్ అవసరం

journal.cli.talent.facet.malformed = { $path } లో ఆకృతి తప్పు డేటా

journal.cli.talent.facet.malformed.line = { $path } లో పంక్తి { $line } వద్ద ఆకృతి తప్పు డేటా

journal.cli.talent.facet.object = ఫాసెట్ డిక్లరేషన్ ఆబ్జెక్ట్ కాదు: { $path }

journal.cli.talent.floor.invalid.budget = { $budget } లేదు లేదా చెల్లనిది

journal.cli.talent.floor.mismatch = { $entries ->
    [one] ఎంట్రీ సంఖ్య సరిపోలేదు: { $entries } ఎంట్రీ పార్స్ అయింది, కానీ { $sources ->
            [one] { $sources } ఎంట్రీ మూలం ఉంది
           *[other] { $sources } ఎంట్రీ మూలాలు ఉన్నాయి
        }
   *[other] ఎంట్రీ సంఖ్య సరిపోలేదు: { $entries } ఎంట్రీలు పార్స్ అయ్యాయి, కానీ { $sources ->
            [one] { $sources } ఎంట్రీ మూలం ఉంది
           *[other] { $sources } ఎంట్రీ మూలాలు ఉన్నాయి
        }
}

journal.cli.talent.help.list = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $line_break }{ $syntax_continuation_5 }{ $line_break }                        షెడ్యూల్ రకంతో ఫిల్టర్ చేయండి{ $line_break }{ $syntax_continuation_7 }{ $line_break }                        ఆరిజిన్‌తో ఫిల్టర్ చేయండి{ $line_break }{ $indent_2 }{ $disabled_option }{ $gap_12 }డిసేబుల్ ప్రాంప్ట్‌లను చేర్చండి{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }{ $jsonl } గా అవుట్‌పుట్

journal.cli.talent.help.log = usage: { $syntax }{ $line_break }{ $line_break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $line_break }  { $id }ఏజెంట్ ఐడి{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_2 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $json_option }{ $gap_6 }ముడి { $jsonl } అవుట్‌పుట్{ $line_break }{ $indent_2 }{ $full_option }{ $gap_6 }ఈవెంట్ వివరాలను విస్తరించండి

journal.cli.talent.help.logs = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }{ $line_break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $line_break }  { $agent }ఒక నిర్దిష్ట ఏజెంట్‌కు ఫిల్టర్ చేయండి{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $line_break }{ $syntax_continuation_9 }{ $line_break }                        చూపాల్సిన రన్‌ల సంఖ్య (డిఫాల్ట్: 20){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }ఈ రోజు రన్‌లను మాత్రమే చూపండి{ $line_break }{ $indent_2 }{ $daily_option }{ $gap_15 }రోజువారీ షెడ్యూల్ రన్‌లను మాత్రమే చూపండి{ $line_break }{ $indent_2 }{ $errors_option }{ $gap_14 }లోపం రన్‌లను మాత్రమే చూపండి{ $line_break }{ $indent_2 }{ $summary_option }{ $gap_13 }గుంపు సారాంశం చూపండి

journal.cli.talent.help.passages = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }                           name{ $line_break }{ $line_break }కార్యకలాప టాలెంట్ చదివే మూలం నుండి పాసేజీలను యథాతథంగా కాపీ చేయండి.{ $line_break }చిన్న మూల యూనిట్ మోడల్ కాల్ లేకుండా మొత్తంగా ఉంచబడుతుంది; పొడవైనది{ $line_break }ఏమి కాపీ చేయాలో ఎంచుకోవడానికి ఒక కంప్లీషన్ తీసుకుంటుంది. జర్నల్‌కు పాసేజీలు ఏవీ{ $line_break }రాయబడవు; కంప్లీషన్ వినియోగం మిగతా వాటిలాగే లాగ్ అవుతుంది.{ $line_break }{ $line_break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $line_break }  { $name }work లేదా conversation వంటి కార్యకలాప టాలెంట్ పేరు{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }పాసేజీ డాక్యుమెంట్‌ను { $json } గా అవుట్‌పుట్ చేయండి{ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }కార్యకలాపం రోజు{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }కార్యకలాపం ఫాసెట్{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }కార్యకలాపం ఐడి

journal.cli.talent.help.root = usage: { $syntax }{ $line_break }{ $line_break }టాలెంట్ ప్రాంప్ట్ కాన్ఫిగ్‌లను పరిశీలించండి{ $line_break }{ $line_break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $line_break }{ $syntax_continuation_5 }{ $line_break }    { $list }షెడ్యూల్ ప్రకారం గుంపు చేసిన ప్రాంప్ట్‌ల జాబితా{ $line_break }    { $show }ఒక నిర్దిష్ట ప్రాంప్ట్ వివరాలు చూపండి{ $line_break }    { $passages }కార్యకలాపం మూలం నుండి మూల పాసేజీలను కాపీ చేయండి{ $line_break }    { $logs }ఇటీవలి టాలెంట్ రన్ లాగ్ చూపండి{ $line_break }    { $log }ఏజెంట్ రన్ ఈవెంట్‌లు చూపండి{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $verbose_option }{ $gap_9 }వివరమైన అవుట్‌పుట్ ఎనేబుల్ చేయండి{ $line_break }{ $indent_2 }{ $debug_option }{ $gap_11 }డీబగ్ లాగింగ్ ఎనేబుల్ చేయండి

journal.cli.talent.help.show = usage: { $syntax }{ $line_break }{ $syntax_continuation_1 }{ $line_break }{ $syntax_continuation_2 }{ $line_break }                           name{ $line_break }{ $line_break }పొజిషనల్ ఆర్గ్యుమెంట్‌లు:{ $line_break }  { $name }ప్రాంప్ట్ పేరు{ $line_break }{ $line_break }ఆప్షన్‌లు:{ $line_break }{ $indent_2 }{ $help_option }{ $gap_12 }ఈ సహాయ సందేశం చూపి నిష్క్రమించండి{ $line_break }{ $indent_2 }{ $json_option }{ $gap_16 }{ $jsonl } గా అవుట్‌పుట్{ $line_break }{ $indent_2 }{ $prompt_option }{ $gap_14 }పూర్తి ప్రాంప్ట్ సందర్భం చూపండి (డ్రై-రన్ మోడ్){ $line_break }{ $indent_2 }{ $day_option }{ $gap_8 }ప్రాంప్ట్ సందర్భం కోసం రోజు{ $line_break }{ $indent_2 }{ $segment_option }{ $gap_2 }సెగ్మెంట్ షెడ్యూల్ ప్రాంప్ట్‌ల కోసం సెగ్మెంట్{ $line_break }{ $indent_2 }{ $facet_option }{ $gap_10 }బహుళ ఫాసెట్ ప్రాంప్ట్‌ల కోసం ఫాసెట్{ $line_break }{ $indent_2 }{ $activity_option }{ $gap_9 }కార్యకలాప షెడ్యూల్ ప్రాంప్ట్‌ల కోసం కార్యకలాపం ఐడి{ $line_break }{ $indent_2 }{ $query_option }{ $gap_10 }టూల్ ఏజెంట్‌ల కోసం ఉదాహరణ క్వెరీ{ $line_break }{ $indent_2 }{ $full_option }{ $gap_16 }కత్తిరింపు లేకుండా పూర్తి కంటెంట్ చూపండి

journal.cli.talent.hook.briefing.date.overflow = బ్రీఫింగ్ తేదీ ఓవర్‌ఫ్లో

journal.cli.talent.hook.briefing.failed = ఉదయం బ్రీఫింగ్ ప్రీ-Hook విఫలమైంది: { $error }

journal.cli.talent.hook.budget = సిద్ధం చేసిన అభ్యర్థన కనిష్ఠ ఇన్‌పుట్ బడ్జెట్‌లో ఇమడదు

journal.cli.talent.hook.daily.lookback = రోజువారీ షెడ్యూల్ వెనక్కి చూపు { $range } బయట ఉంది

journal.cli.talent.hook.day.invalid = చెల్లని లేదా పార్స్ చేయలేని రోజు: '{ $day }'

journal.cli.talent.hook.day.missing = రోజు లేదు

journal.cli.talent.hook.document.stream = డాక్యుమెంట్ ఇంపోర్ట్ సెగ్మెంట్ కాదు

journal.cli.talent.hook.entity.minimum = { $entity } కోసం ఎంటిటీ ప్యాకెట్ కనీసం { $count } అక్షరాలు; గరిష్ఠం { $maximum }

journal.cli.talent.hook.entity.name = ఎంటిటీ పేరు లేదు

journal.cli.talent.hook.entity.size = { $entity } కోసం ఎంటిటీ ప్యాకెట్ { $count } అక్షరాలు; గరిష్ఠం { $maximum }

journal.cli.talent.hook.facet.missing = ఫాసెట్ లేదు

journal.cli.talent.hook.facet.read = ఫాసెట్ { $facet } చదవలేకపోయింది: { $error }

journal.cli.talent.hook.facets.list = ఫాసెట్‌లను జాబితా చేయలేకపోయింది: { $error }

journal.cli.talent.hook.facts.size = ఎంటిటీ ఫ్యాక్ట్‌ల సందర్భం { $count } అక్షరాలు; గరిష్ఠం { $maximum }

journal.cli.talent.hook.failure = { $phase } hook '{ $stage }' టాలెంట్ '{ $talent }' కోసం: { $detail }

journal.cli.talent.hook.identities.read = ఎంటిటీ గుర్తింపులను చదవలేకపోయింది: { $error }

journal.cli.talent.hook.identity.object = ఎంటిటీ గుర్తింపు ఆబ్జెక్ట్ కాదు: { $path }

journal.cli.talent.hook.lookup.control = చెల్లని పాత్ కాంపోనెంట్ '{ $candidate }': పేరులో కంట్రోల్ అక్షరం ఉంది

journal.cli.talent.hook.lookup.dotcomponent = చెల్లని పాత్ కాంపోనెంట్ '{ $candidate }': { $dot } మరియు { $parent } చెల్లుబాటు అయ్యే పేర్లు కావు

journal.cli.talent.hook.lookup.empty = చెల్లని పాత్ కాంపోనెంట్ '{ $candidate }': పేరు ఖాళీ

journal.cli.talent.hook.lookup.kind = { $path } డైరెక్టరీ కాదు ({ $kind })

journal.cli.talent.hook.lookup.layout = డైరెక్ట్ లేఅవుట్‌కు ప్రవాహం "{ $default_stream }" అవసరం, వచ్చింది { $stream }

journal.cli.talent.hook.lookup.rootorprefix = చెల్లని పాత్ కాంపోనెంట్ '{ $candidate }': పేరు ఒకే సాపేక్ష కాంపోనెంట్ అయి ఉండాలి

journal.cli.talent.hook.lookup.separator = చెల్లని పాత్ కాంపోనెంట్ '{ $candidate }': పేరులో పాత్ సెపరేటర్ ఉంది

journal.cli.talent.hook.possible.no.prompt = టాలెంట్‌కు ప్రాంప్ట్ లేదు

journal.cli.talent.hook.resolution.absent = పరిష్కరించిన అయోమయం { $ambiguity } ఈ పరిష్కార కాల్‌లో లేని ఎంటిటీని పేర్కొంటుంది: { $entity }

journal.cli.talent.hook.resolution.hidden = పరిష్కరించిన అయోమయం { $ambiguity } దాచబడిన ఎంటిటీని పేర్కొంటుంది: { $entity }

journal.cli.talent.hook.review.changed = సిద్ధం చేసే సమయంలో సమీక్ష ఇన్‌పుట్‌లు మారాయి

journal.cli.talent.hook.stream.layout = డైరెక్ట్ లేఅవుట్‌కు ప్రవాహం "{ $default_stream }" అవసరం, వచ్చింది "{ $stream }"

journal.cli.talent.hook.suggest.size = ఎంటిటీ సూచన సందర్భం { $count } అక్షరాలు; గరిష్ఠం { $maximum }

journal.cli.talent.hook.suggestions.array = సూచనల JSON లో { $entities } అర్రే లేదు

journal.cli.talent.hook.suggestions.json = { $path } లో చెల్లని సూచనల JSON: { $error }

journal.cli.talent.hook.suggestions.missing = సూచనల ఆర్టిఫ్యాక్ట్ లేదు: { $path }

journal.cli.talent.hook.sunday = అభ్యర్థన రోజు { $day } ఆదివారం కాదు

journal.cli.talent.hook.transcript.bytes = ట్రాన్స్క్రిప్ట్ బైట్‌లు చెల్లుబాటు అయ్యే UTF-8 కావు

journal.cli.talent.hook.transcript.read = ట్రాన్స్క్రిప్ట్ { $file } చదవలేకపోయింది: { $error }

journal.cli.talent.hook.transcript.utf8 = ట్రాన్స్క్రిప్ట్ { $file } UTF-8 కాదు: { $error }

journal.cli.talent.hook.week.incomplete = { $end } తో ముగిసే వారం ఈ రోజు { $today } కంటే ముందు పూర్తి కాలేదు

journal.cli.talent.last.age = { $age } క్రితం

journal.cli.talent.last.age.duration = { $age } క్రితం ({ $duration })

journal.cli.talent.list.empty = ఫిల్టర్‌లకు సరిపోయే ప్రాంప్ట్‌లు ఏవీ కనబడలేదు.

journal.cli.talent.list.group.activity = activity:

journal.cli.talent.list.group.cadence = cadence:

journal.cli.talent.list.group.daily = daily:

journal.cli.talent.list.group.segment = segment:

journal.cli.talent.list.group.unscheduled = unscheduled:

journal.cli.talent.list.group.weekly = weekly:

journal.cli.talent.list.header.last = LAST RUN

journal.cli.talent.list.header.name = NAME

journal.cli.talent.list.header.tags = ట్యాగ్‌లు

journal.cli.talent.list.header.title = శీర్షిక

journal.cli.talent.list.tag.disabled = డిసేబుల్

journal.cli.talent.list.tag.fail = విఫలమైంది

journal.cli.talent.list.visible = { $count ->
    [one] { $count } ప్రాంప్ట్ ({ $hidden } డిసేబుల్ దాచబడింది, { $disabled_flag } వాడండి)
   *[other] { $count } ప్రాంప్ట్‌లు ({ $hidden } డిసేబుల్ దాచబడింది, { $disabled_flag } వాడండి)
}

journal.cli.talent.log.event.thinking = think

journal.cli.talent.log.event.tool = tool

journal.cli.talent.log.event.updated = updated

journal.cli.talent.log.missing = టాలెంట్ రన్ కనబడలేదు: { $run }

journal.cli.talent.log.tokens = { $result } [{ $input }in/{ $output }out]

journal.cli.talent.logs.date = { $month } { $day_time }

journal.cli.talent.logs.day.invalid = చెల్లని { $day_flag } ఫార్మాట్: { $day }. ఊహించినది { $day_format }.

journal.cli.talent.logs.total = మొత్తం

journal.cli.talent.outcome.facet.write = FacetEntityWriteError: { $error }

journal.cli.talent.outcome.object = ఫలితం JSON ఆబ్జెక్ట్ కాదు

journal.cli.talent.outcome.observation.write = ObservationWriteError: { $error }

journal.cli.talent.outcome.observer.entities = ఎంటిటీలు జాబితా కాదు

journal.cli.talent.outcome.observer.parse = ఫలితాన్ని JSON గా పార్స్ చేయలేకపోయింది

journal.cli.talent.outcome.review.arrays = ఫలితంలో ఊహించిన అర్రేలు లేవు

journal.cli.talent.outcome.review.candidate = EntityReviewCandidateError: { $error }

journal.cli.talent.outcome.review.parse = చెల్లని JSON

journal.cli.talent.passages.generate = మూల పాసేజీలు generate కు చేరలేకపోయాయి: { $error }

journal.cli.talent.passages.generate.missing = మూల పాసేజీలు generate కు చేరలేకపోయాయి: సిబ్లింగ్ ఎగ్జిక్యూటబుల్ { $path } లేదు

journal.cli.talent.passages.generate.parent = మూల పాసేజీలు generate కు చేరలేకపోయాయి: ప్రస్తుత ఎగ్జిక్యూటబుల్‌కు పేరెంట్ లేదు

journal.cli.talent.passages.incomplete = కొన్ని మూల యూనిట్‌లకు పాసేజీలు లేవు; ప్రతి యూనిట్ ఫలితం చూడండి

journal.cli.talent.passages.unavailable = మూల పాసేజీలు అందుబాటులో లేవు: { $detail }

journal.cli.talent.passages.unit.failed = # యూనిట్ { $index } · { $kind } · పాసేజీలు లేవు: { $reason }

journal.cli.talent.passages.unit.failed.time = # యూనిట్ { $index } · { $kind } · { $time } · పాసేజీలు లేవు: { $reason }

journal.cli.talent.passages.unit.not.attempted = # యూనిట్ { $index } · { $kind } · ప్రయత్నించలేదు

journal.cli.talent.passages.unit.not.attempted.time = # యూనిట్ { $index } · { $kind } · { $time } · ప్రయత్నించలేదు

journal.cli.talent.passages.unit.passages = { $count ->
    [one] # యూనిట్ { $index } · { $kind } · { $count } పాసేజీ
   *[other] # యూనిట్ { $index } · { $kind } · { $count } పాసేజీలు
}

journal.cli.talent.passages.unit.passages.time = { $count ->
    [one] # యూనిట్ { $index } · { $kind } · { $time } · { $count } పాసేజీ
   *[other] # యూనిట్ { $index } · { $kind } · { $time } · { $count } పాసేజీలు
}

journal.cli.talent.passages.verbatim = మూలం నుండి యథాతథం:

journal.cli.talent.path.absolute = జర్నల్ పాత్ సాపేక్షంగా ఉండాలి

journal.cli.talent.path.backslash = జర్నల్ పాత్ ఫార్వర్డ్ స్లాష్‌లను వాడాలి

journal.cli.talent.path.component = జర్నల్ పాత్‌లో చెల్లని కాంపోనెంట్ ఉంది

journal.cli.talent.path.day = రోజు { $day_format } అయి ఉండాలి

journal.cli.talent.path.empty = జర్నల్ పాత్ ఖాళీగా ఉండకూడదు

journal.cli.talent.path.escape = { $relative } జర్నల్ రూట్‌ను దాటుతుంది (రిజల్వ్ అయినది { $path })

journal.cli.talent.path.unreadable = జర్నల్ పాత్ చదవదగినది కాదు

journal.cli.talent.prepare.configuration.name.declares.an.invalid.mobile.local.timeout = కాన్ఫిగ్ '{ $name }' చెల్లని { $timeout } ప్రకటిస్తుంది

journal.cli.talent.prepare.invalid.config.must.have.type.day.or.prompt = చెల్లని కాన్ఫిగ్: { $type }, { $day }, లేదా { $prompt } ఉండాలి

journal.cli.talent.prepare.invalid.config.segment.or.span.requires.day = చెల్లని కాన్ఫిగ్: { $segment } లేదా { $span } కు { $day } అవసరం

journal.cli.talent.prepare.invalid.talent.type.kind = చెల్లని టాలెంట్ రకం '{ $kind }'

journal.cli.talent.prepare.memory.incomplete = ప్రైవేట్ జ్ఞాపక సందర్భం అసంపూర్ణం

journal.cli.talent.prepare.request.field.field.is.retired = అభ్యర్థన ఫీల్డ్ '{ $field }' రిటైర్ అయినది

journal.cli.talent.prepare.request.overrides.field.for.talent.name.are.not.allowed = అభ్యర్థన ఓవర్‌రైడ్‌లు '{ $field }' టాలెంట్ '{ $name }' కోసం అనుమతి లేదు ({ $declared } != { $requested })

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed = '{ $field }' కోసం అభ్యర్థన ఓవర్‌రైడ్‌లకు అనుమతి లేదు; ఆలోచన ప్రొవైడర్ మరియు మోడల్ ఆలోచనలో మాత్రమే కాన్ఫిగ్ అవుతాయి

journal.cli.talent.prepare.request.overrides.for.field.are.not.allowed.budget = '{ $field }' కోసం అభ్యర్థన ఓవర్‌రైడ్‌లకు అనుమతి లేదు; టాలెంట్ బడ్జెట్‌లు దానివే

journal.cli.talent.prepare.segment.missing = { $count ->
    [one] సెగ్మెంట్ డైరెక్టరీ కనబడలేదు: { $segments }
   *[other] సెగ్మెంట్ డైరెక్టరీలు కనబడలేదు: { $segments }
}

journal.cli.talent.prepare.talent.name.declares.field.in.frontmatter = టాలెంట్ '{ $name }' ఫ్రంట్‌మ్యాటర్‌లో '{ $field }' ప్రకటిస్తుంది; ఆలోచన ప్రొవైడర్ మరియు మోడల్ ఆలోచనలో మాత్రమే కాన్ఫిగ్ అవుతాయి

journal.cli.talent.prepare.talent.name.declares.outbound.approval.in.frontmatter = టాలెంట్ '{ $name }' ఫ్రంట్‌మ్యాటర్‌లో { $outbound_approval } ప్రకటిస్తుంది; ఈ ఫీల్డ్ లాంచ్ కాన్ఫిగ్‌లో మాత్రమే, టాలెంట్ నిర్వచనం నుండి రాకూడదు

journal.cli.talent.prepare.talent.name.not.found = టాలెంట్ '{ $name }' కనబడలేదు

journal.cli.talent.prepare.talent.request.missing.name = టాలెంట్ అభ్యర్థనలో పేరు లేదు

journal.cli.talent.preview.all.lines = ({ $full_flag } వాడి మొత్తం { $count } పంక్తులు చూడండి)

journal.cli.talent.preview.banner = ఈ టాలెంట్ పంపేదాని ప్రివ్యూ. నిజమైన రన్ వాడే అదే అసెంబ్లీ; మోడల్‌ను పిలవదు.

journal.cli.talent.preview.effective = అమలులో ఉన్న ప్రాంప్ట్: { $talent }

journal.cli.talent.preview.empty = (ఖాళీ)

journal.cli.talent.preview.instruction = సూచన

journal.cli.talent.preview.no.day = రోజు ఇవ్వలేదు, కాబట్టి ఈ ప్రివ్యూలో ఆ రోజు రికార్డింగ్‌లు లేవు. వాటిని చేర్చడానికి { $day_flag } { $day_format } ఇవ్వండి.

journal.cli.talent.preview.omitted = { $count ->
    [one] ... ({ $count } పంక్తి మినహాయించబడింది)
   *[other] ... ({ $count } పంక్తులు మినహాయించబడ్డాయి)
}

journal.cli.talent.preview.omitted.legacy = ... ({ $count } పంక్తులు మినహాయించబడ్డాయి)

journal.cli.talent.preview.prestep = ప్రాంప్ట్ ప్రివ్యూ ఈ టాలెంట్‌ను అసెంబ్లీ చేయలేకపోయింది: అవసరమైన ప్రీ-స్టెప్ అందుబాటులో లేదు.

journal.cli.talent.preview.recovery.choose.a.completed.owner.activity.not.an.anticipated.or.cogi = పూర్తయిన మీ కార్యకలాపాన్ని ఎంచుకోండి, అంచనా లేదా కాగిటేట్ నమోదును కాదు.

journal.cli.talent.preview.recovery.choose.a.talent.configured.for.this.activity.kind = ఈ కార్యకలాప రకానికి కాన్ఫిగ్ అయిన టాలెంట్‌ను ఎంచుకోండి.

journal.cli.talent.preview.recovery.choose.a.talent.that.loads.its.activity.s.sources = తన కార్యకలాపం మూలాలను లోడ్ చేసే టాలెంట్‌ను ఎంచుకోండి.

journal.cli.talent.preview.recovery.choose.a.talent.that.reads.its.activity.source.without.a.pre = ప్రీ-స్టెప్ లేకుండా తన కార్యకలాప మూలాన్ని చదివే టాలెంట్‌ను ఎంచుకోండి.

journal.cli.talent.preview.recovery.choose.a.talent.whose.required.pre.step.is.available.in.the = అవసరమైన ప్రీ-స్టెప్ స్థానిక రన్‌టైమ్‌లో అందుబాటులో ఉన్న టాలెంట్‌ను ఎంచుకోండి.

journal.cli.talent.preview.recovery.choose.a.talent.whose.schedule.is.activity = షెడ్యూల్ { $schedule } అయిన టాలెంట్‌ను ఎంచుకోండి.

journal.cli.talent.preview.recovery.choose.an.activity.id.stored.in.the.requested.day.and.facet = అడిగిన రోజు మరియు ఫాసెట్‌లో నిల్వ ఉన్న కార్యకలాపం ఐడిని ఎంచుకోండి.

journal.cli.talent.preview.recovery.choose.an.activity.with.at.least.one.recorded.segment = కనీసం ఒక నమోదు అయిన సెగ్మెంట్ ఉన్న కార్యకలాపాన్ని ఎంచుకోండి.

journal.cli.talent.preview.recovery.choose.either.activity.or.segment.not.both = { $activity_flag } లేదా { $segment_flag } ఎంచుకోండి, రెండూ కాదు.

journal.cli.talent.preview.recovery.correct.the.activity.preview.input.then.retry.detail = కార్యకలాప ప్రివ్యూ ఇన్‌పుట్‌ను సరిచేసి మళ్ళీ ప్రయత్నించండి: { $detail }

journal.cli.talent.preview.recovery.correct.the.talent.configuration.detail = టాలెంట్ కాన్ఫిగ్‌ను సరిచేయండి: { $detail }

journal.cli.talent.preview.recovery.make.the.selected.activity.and.talent.eligible.then.retry = ఎంచుకున్న కార్యకలాపాన్ని మరియు టాలెంట్‌ను అర్హం చేసి మళ్ళీ ప్రయత్నించండి.

journal.cli.talent.preview.recovery.make.the.selected.talent.eligible.then.retry = ఎంచుకున్న టాలెంట్‌ను అర్హం చేసి మళ్ళీ ప్రయత్నించండి.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.facet.name.and.activity.id = { $day_flag } { $yyyymmdd }, { $facet_flag } { $name } మరియు { $activity_flag } ఐడి ఇవ్వండి.

journal.cli.talent.preview.recovery.pass.day.yyyymmdd.with.activity = { $day_flag } { $yyyymmdd }, { $activity_flag } తో ఇవ్వండి.

journal.cli.talent.preview.recovery.pass.facet.name.with.activity = { $facet_flag } { $name }, { $activity_flag } తో ఇవ్వండి.

journal.cli.talent.preview.recovery.remove.duplicate.records.with.this.id.in.the.requested.day.a = అడిగిన రోజు మరియు ఫాసెట్‌లో ఈ ఐడి ఉన్న నకలు నమోదులను తీసివేయండి.

journal.cli.talent.preview.recovery.repair.the.activity.span.so.every.segment.is.a.non.empty.str = ప్రతి సెగ్మెంట్ ఖాళీ కాని స్ట్రింగ్ అయేలా కార్యకలాప వ్యవధిని బాగుచేయండి.

journal.cli.talent.preview.recovery.restore.a.readable.activity.record.file.then.retry.detail = చదవగలిగే కార్యకలాప నమోదు ఫైల్‌ను రీస్టోర్ చేసి మళ్ళీ ప్రయత్నించండి: { $detail }

journal.cli.talent.preview.recovery.restore.a.readable.segment.directory.then.retry.detail = చదవగలిగే సెగ్మెంట్ డైరెక్టరీని రీస్టోర్ చేసి మళ్ళీ ప్రయత్నించండి: { $detail }

journal.cli.talent.preview.recovery.restore.readable.segment.directories.then.retry.detail = చదవగలిగే సెగ్మెంట్ డైరెక్టరీలను రీస్టోర్ చేసి మళ్ళీ ప్రయత్నించండి: { $detail }

journal.cli.talent.preview.recovery.restore.the.selected.activity.segment.then.retry = ఎంచుకున్న కార్యకలాప సెగ్మెంట్‌ను రీస్టోర్ చేసి మళ్ళీ ప్రయత్నించండి.

journal.cli.talent.preview.skipped = ఈ టాలెంట్ నడిచేది కాదు: { $reason }

journal.cli.talent.show.body = body:

journal.cli.talent.show.body.lines = { $count ->
    [one] { $count } పంక్తి
   *[other] { $count } పంక్తులు
}

journal.cli.talent.show.hook = ప్రాంప్ట్ '{ $talent }' ఒక hook ప్రాంప్ట్, దాన్ని నేరుగా నడపలేం.

journal.cli.talent.show.missing = ప్రాంప్ట్ కనబడలేదు: { $talent }

journal.cli.talent.show.missing.path = ప్రాంప్ట్ కనబడలేదు: { $talent }{ $line_break }  చూసినది: { $path }

journal.cli.talent.show.variables = variables:

journal.cli.talent.startup.executable = టాలెంట్ విఫలమైంది: ప్రస్తుత ఎగ్జిక్యూటబుల్‌ను పరిశీలించలేకపోయింది: { $error }

journal.cli.talent.startup.package = టాలెంట్ విఫలమైంది: ఇన్‌స్టాల్ అయిన { $package } ప్యాకేజీని కనుగొనలేకపోయింది

journal.cli.talent.work.duration = చెల్లని Work మూల కాలవ్యవధి: { $error }

journal.cli.talent.work.duration.range = Work మూల కాలవ్యవధి పరిధి బయట ఉంది

journal.cli.talent.work.interval = చెల్లని Work మూల విరామం: { $error }

journal.cli.talent.work.interval.range = Work మూల విరామం పరిధి బయట ఉంది

journal.cli.talent.work.key = చెల్లని Work మూల సెగ్మెంట్: { $segment }

journal.cli.talent.work.read = Work సెగ్మెంట్ ఫలితాలను చదవలేకపోయింది: { $error }

journal.cli.talent.work.unavailable = Work మూల సెగ్మెంట్ అందుబాటులో లేదు: { $day }/{ $segment }

journal.cli.top.crashed.attempts = { $name } (ప్రయత్నాలు: { $attempts })

journal.cli.top.crashed.crashed = { $name } (క్రాష్ అయింది; ప్రయత్నాలు: { $attempts })

journal.cli.top.crashed.heading.attention = శ్రద్ధ కావాల్సిన సేవలు:

journal.cli.top.crashed.heading.crashed = క్రాష్ అయింది:

journal.cli.top.crashed.retrying = { $name } (మళ్ళీ ప్రయత్నిస్తోంది; ప్రయత్నాలు: { $attempts })

journal.cli.top.error.apply = టర్మినల్ అమలు విఫలమైంది: { $detail }

journal.cli.top.error.journal.config = జర్నల్ కాన్ఫిగ్: { $detail }

journal.cli.top.error.screen = టర్మినల్ స్క్రీన్ ప్రవేశం విఫలమైంది: { $detail }

journal.cli.top.error.stdin.not.tty = stdin టర్మినల్ కాదు

journal.cli.top.error.stdout.not.tty = stdout టర్మినల్ కాదు

journal.cli.top.error.terminal = టర్మినల్ వైఫల్యం: { $detail }

journal.cli.top.error.transport = ట్రాన్స్‌పోర్ట్ వైఫల్యం: { $detail }

journal.cli.top.error.width = టర్మినల్ వెడల్పు విఫలమైంది: { $detail }

journal.cli.top.error.width.unavailable = టర్మినల్ వెడల్పు అందుబాటులో లేదు

journal.cli.top.footer.empty = q: నిష్క్రమించండి

journal.cli.top.footer.multi = ↑/↓: నావిగేట్ చేయండి  r: రీస్టార్ట్ చేయండి  q: నిష్క్రమించండి

journal.cli.top.footer.single = r: రీస్టార్ట్ చేయండి  q: నిష్క్రమించండి

journal.cli.top.header.last.log = చివరి లాగ్

journal.cli.top.header.mb = MB

journal.cli.top.header.percent = %

journal.cli.top.header.pid = PID

journal.cli.top.header.runtime = రన్‌టైమ్

journal.cli.top.header.service = సేవ

journal.cli.top.header.task = పని

journal.cli.top.header.uptime = అప్‌టైమ్

journal.cli.top.malformed.events = ఆకృతి తప్పు ఈవెంట్‌లు: { $count } ({ $reason })

journal.cli.top.observe.describe = describe

journal.cli.top.observe.idle = [పని లేకుండా]

journal.cli.top.observe.idle.locked = [పని లేకుండా] లాక్ అయి ఉంది

journal.cli.top.observe.live.badge = [LIVE]

journal.cli.top.observe.recent = ఇటీవలి: { $segments }

journal.cli.top.observe.screencast = స్క్రీన్‌కాస్ట్ { $elapsed }

journal.cli.top.observe.tmux.badge = [TMUX]

journal.cli.top.observe.tmux.captures = { $quantity ->
    [one] { $count } ఫ్రేమ్
   *[other] { $count } ఫ్రేమ్‌లు
}

journal.cli.top.observe.transcribe = ట్రాన్స్క్రైబ్

journal.cli.top.observe.voice = గొంతు { $hits }

journal.cli.top.queued = వరుసలో: { $commands }

journal.cli.top.reconnecting = (మళ్ళీ కనెక్ట్ అవుతోంది)

journal.cli.top.section.observe = పరిశీలన

journal.cli.top.section.think = ఆలోచన

journal.cli.top.section.thinking.health = ఆలోచన స్థితి

journal.cli.top.task.status.failed = విఫలమైంది

journal.cli.top.task.status.gone = పోయింది

journal.cli.top.task.status.ok = ok

journal.cli.top.think.agents = { $total_quantity ->
    [one] { $completed }/{ $total } రన్
   *[other] { $completed }/{ $total } రన్‌లు
}

journal.cli.top.think.last.failed = { $failed } విఫలమైంది

journal.cli.top.think.last.prefix = చివరి: { $success } ok

journal.cli.top.think.segments = సెగ్మెంట్ { $completed }/{ $total }

journal.cli.top.thinking.checking = (సరిచూస్తోంది)

journal.cli.top.thinking.unavailable = (స్థితి అందుబాటులో లేదు)

journal.cli.top.title = solstone కార్యకలాప మేనేజర్

journal.cli.top.unknown = తెలియదు

journal.cli.top.waiting.services = (సేవల కోసం వేచి ఉంది)

journal.cli.top.waiting.status = (స్థితి కోసం వేచి ఉంది)

journal.cli.top.waiting.think = (ఆలోచన కోసం వేచి ఉంది)
