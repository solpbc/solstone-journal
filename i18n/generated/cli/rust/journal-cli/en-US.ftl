m6a6f75726e616c2e636c692e61626f75742e6f732e72656c656173652e756e617661696c61626c65 = about: { "os-release" } unavailable: { $error }

m6a6f75726e616c2e636c692e61626f75742e7265736f757263652e756e617661696c61626c65 = the journal's about information couldn't be read. check that your journal is running, then try again.

m6a6f75726e616c2e636c692e61626f75742e75736167652e6a6f75726e616c = usage: { "solstone journal about [-h | --help]" }

m6a6f75726e616c2e636c692e61626f75742e75736167652e72656d6f7465 = usage: { "solstone about [-h | --help]" }

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e6d616c666f726d6564 = the journal response couldn't be read.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e74696d656f7574 = The journal didn't answer in time.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e756e726561636861626c65 = your journal couldn't be reached over { "HTTP" }.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e756e7265616461626c65 = The journal returned an unreadable error.

m6a6f75726e616c2e636c692e636c69656e742e706f72742e696e76616c6964 = { "SOLSTONE_CONVEY_PORT" } must be an integer from 1 to 65535.

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e62616e64 = { $i18nCategory ->
    [one] band { $band }: { $recordings } recording ready / { $recordings_left } left, { $outputs } daily { $i18nCategory-output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $i18nCategory-activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $i18nCategory-request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
   *[other] band { $band }: { $recordings } recordings ready / { $recordings_left } left, { $outputs } daily { $i18nCategory-output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $i18nCategory-activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $i18nCategory-request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e636175676874 = caught up

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6361756768742e636170706564 = { $i18nCategory ->
    [one] caught up; { $days } day completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] caught up; { $days } day(s) completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6368616e67652e7265636f72646564 = { $name } recorded a change on { $location }; resolve the recorded change before that day is redone

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6368616e67652e73746172746564 = { $name } may have started a change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e636865636b6564 = recent work checked

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e636865636b65642e636170706564 = { $i18nCategory ->
    [one] recent work checked; { $days } day completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] recent work checked; { $days } day(s) completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6465666572726564 = { $name } stopped on { $location }; it can retry on a later run after the local day changes

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e64656665727265642e726561736f6e = { $name } stopped on { $location } with reason { $reason }; it can retry on a later run after the local day changes

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e656c696769626c65 = { $name } stopped on { $location }; it can retry on the next eligible run

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e656c696769626c652e726561736f6e = { $name } stopped on { $location } with reason { $reason }; it can retry on the next eligible run

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e656d7074792e64617465 = none

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e657868617573746564 = { $name } stopped on { $location } after its automatic retry; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6578686175737465642e726561736f6e = { $name } stopped on { $location } with reason { $reason } after its automatic retry; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e686f6c642e7265747279 = { $day }: { $reason } (retry { $unit })

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e7061727469616c2e63757272656e74 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e7061727469616c2e6c617374 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e77686f6c652e63757272656e74 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e77686f6c652e6c617374 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e70656e64696e67 = pending: { $days }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e70656e64696e672e636f756e7473 = { $i18nCategory ->
    [one] { $pending } day pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
   *[other] { $pending } day(s) pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e70656e64696e672e6f6c64657374 = { $i18nCategory ->
    [one] { $pending } day pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
   *[other] { $pending } day(s) pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265736f6c7665 = { $name } stopped on { $location }; resolve the conflict before reprocessing a past day

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265736f6c76652e726561736f6e = { $name } stopped on { $location } with reason { $reason }; resolve the conflict before reprocessing a past day

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265747279 = { $name } stopped on { $location }; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e72657472792e726561736f6e = { $name } stopped on { $location } with reason { $reason }; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265766965772e73746172746564 = { $name } may have started an entity review change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e737475636b = stuck: { $days }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e756e66696e6973686564 = { $i18nCategory ->
    [one] { $activities } activity on { $days } completed { $i18nCategory-day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
   *[other] { $activities } activities on { $days } completed { $i18nCategory-day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e756e6b6e6f776e = unknown: { $days }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e756e6b6e6f776e2e636f756e74 = { $i18nCategory ->
    [one] couldn't fully determine; { $days } day unknown
   *[other] couldn't fully determine; { $days } day(s) unknown
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v756e626f756e64 = setup association unknown; history only

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v756e6b6e6f776e = current setup association unavailable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v73757065727365646564 = superseded setup; history only

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v63757272656e74 = matches the selected setup

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v696e76616c6964 = recorded setup association unavailable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e696e636f6d706c657465_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; some files were not copied; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e696e636f6d706c657465_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; some files were not copied; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e696e636f6d706c657465_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; some files were not copied; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6c6f636b6564_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; repository was locked; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6c6f636b6564_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; repository was locked; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6c6f636b6564_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; repository was locked; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6f7264696e617279_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6f7264696e617279_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6f7264696e617279_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e72656675736564_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; backup key refused; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e72656675736564_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; backup key refused; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e72656675736564_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; backup key refused; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e74696d656f7574_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; timed out; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e74696d656f7574_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; timed out; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e74696d656f7574_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; timed out; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e756e6b6e6f776e_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; cause unknown; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e756e6b6e6f776e_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; cause unknown; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e756e6b6e6f776e_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; cause unknown; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e646174652e756e6b6e6f776e = unknown date

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e64657461696c73 = open backup or health in your journal for details

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e696e636f6d706c6574652e6461746564 = { $i18nCategory ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; incomplete copy, some files were not copied; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; incomplete copy, some files were not copied; present coverage and verification unknown
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e696e636f6d706c6574652e756e6461746564 = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; incomplete copy, some files were not copied; present coverage and verification unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e737563636573732e6461746564 = { $i18nCategory ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; present coverage and verification unknown
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e737563636573732e756e6461746564 = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; present coverage and verification unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e756e6b6e6f776e = last successful copy unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e696e76616c69642e686973746f7279 = backup on; recorded status unavailable; { $history }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7061757365642e686973746f7279 = backup paused by you since { $date }; { $history }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e736e617073686f742e756e6b6e6f776e = unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e696e76616c69642e7061757365 = backup status unavailable: invalid pause setting

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e696e76616c69642e73657474696e6773 = backup status unavailable: invalid backup settings

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e696e76616c69642e737769746368 = backup status unavailable: invalid on/off setting

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e6e6f2e617474656d7074 = backup on; no recorded attempt

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e6f6666 = backup off

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e756e617661696c61626c652e6a6f75726e616c = backup status unavailable: journal unreadable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e756e617661696c61626c652e73657474696e6773 = backup status unavailable: settings unreadable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e756e636f6e66696775726564 = backup not configured

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e6167652e756e6b6e6f776e = unknown

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e6e6f2e636f6d706f6e656e74 = none

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e7265706f7274 = { $headline }; { "state" }={ $state }; { "reason" }={ $reason }; { "component" }={ $component }; { "evidence_age" }={ $age }

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e756e6b6e6f776e = unknown: brain state unavailable

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e756e6b6e6f776e2e726561736f6e = unknown: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e616374697665 = { "rollup=active" }; the { "solstone" } app on every device that has added to your journal is current

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e646174612e756e617661696c61626c65 = { "rollup=unknown" }; device activity unavailable

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e696e7370656374 = open { "/app/health" } to inspect each device

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e6e6f2e73656e64657273 = { "rollup=no_senders" }; the { "solstone" } app hasn't added anything to your journal yet

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e7175696574 = { "rollup=quiet" }; no rejected uploads recorded; see devices for last delivery times

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e72656769737472792e756e617661696c61626c65 = { "rollup=unknown" }; device list unavailable

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e726563656e74 = the { "solstone" } app on { $client } last added { $hours }h ago

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e726563656e742e736f7572636573 = { $i18nCategory ->
    [one] the { "solstone" } app on { $client } last added { $hours }h ago ({ $sources } needs attention)
   *[other] the { "solstone" } app on { $client } last added { $hours }h ago ({ $sources } need attention)
}

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e726f6c6c7570 = { "rollup=attention" }; { $devices }

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e736f7572636573 = the { "solstone" } app on { $client } is having trouble adding { $sources }

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e74726f75626c65 = the { "solstone" } app on { $client } is having trouble adding

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e74726f75626c652e736f7572636573 = { $i18nCategory ->
    [one] the { "solstone" } app on { $client } is having trouble adding ({ $sources } needs attention)
   *[other] the { "solstone" } app on { $client } is having trouble adding ({ $sources } need attention)
}

m6a6f75726e616c2e636c692e646f63746f722e636865636b2e6465666572726564 = deferred check set: { $set }

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e62696e64696e672e616374697665 = active client records={ $records }; unbound=0; { "certificate cid" } binding is authoritative

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e61637469766974792e756e617661696c61626c65 = device activity unavailable

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e696e7370656374 = open { "/app/health" } to inspect the rejected upload

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e6c6973742e756e617661696c61626c65 = device list unavailable

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e6e6f2e73656e64657273 = the { "solstone" } app hasn't added anything to your journal yet

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e7175696574 = no rejected uploads recorded; see devices for last delivery times

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e696e676573742e6865616c74682e6865616c746879 = no devices failing ingest

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e696e676573742e6865616c74682e72656a656374696f6e = device { $device } failing ingest: { $reason }, { $attempts }x since { $since }

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e696e676573742e6865616c74682e726570616972 = update or restart the device, then confirm a valid upload clears the rejection

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e7472616e73706f72742e7265667573616c2e6865616c746879 = no devices had requests turned away in the last 7 days

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e7472616e73706f72742e7265667573616c2e726570616972 = this is a busy connection, not a lost one; report it if it keeps happening

m6a6f75726e616c2e636c692e646f63746f722e636c69656e74732e6c65646765722e756e617661696c61626c65 = authorized client ledger unavailable: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e636f6e6669672e756e7772697461626c65 = service config directory is not writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e636f6e6669672e77696c6c2e637265617465 = home is writable; install will create { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e636f6e6669672e7772697461626c65 = home and service config dir are writable ({ $path })

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e686f6d652e6d697373696e67 = home directory does not exist: { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e686f6d652e756e757361626c65 = home directory is not readable and writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e6f776e65722e756e617661696c61626c65 = could not locate the owner base: { $error }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e6f776e65722e77696e646f7773 = the Windows owner base is only readable on Windows

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e726573746f7265 = repair or restore { "config/journal.json" } from a backup

m6a6f75726e616c2e636c692e646f63746f722e636f6e746578742e6661696c6564 = doctor failed to resolve context: { $error }

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e6261636b656e64 = configured backend is { $backend }; parakeet readiness not applicable

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e646f776e6c6f6164 = { "CoreML" } parakeet model is not downloaded — fetch it with: { "solstone journal install-models" }

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e6f746865722e6d6f64656c2e7265616479 = parakeet model ready at { $path }

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e756e737570706f72746564 = parakeet not supported on this platform

m6a6f75726e616c2e636c692e646f63746f722e64656c69766572792e72656a6563746564 = the journal rejected an upload from { $client }

m6a6f75726e616c2e636c692e646f63746f722e64656c69766572792e72656a65637465642e736f7572636573 = { $i18nCategory ->
    [one] the journal rejected an upload from { $client } ({ $sources } needs attention)
   *[other] the journal rejected an upload from { $client } ({ $sources } need attention)
}

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e6661756c74 = day { $day } stream { $stream } device { $device }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e7265616461626c65 = the journal can read the last { $days } days for every bound device

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e726570616972 = a device syncing that day is told the journal cannot read it; a { "solstone" } app older than the 2026-09 fix shows that as offline; inspect the named stream directory's segments and their { "events.jsonl" }

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e73747265616d732e756e617661696c61626c65 = device streams unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e73747265616d732e756e626f756e64 = no device streams bound

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e7265667573616c2e636f756e74 = { $i18nCategory ->
    [one] device { $device } had a request turned away: { $reason }, { $attempts }x through { $last_seen }
   *[other] device { $device } had requests turned away: { $reason }, { $attempts }x through { $last_seen }
}

m6a6f75726e616c2e636c692e646f63746f722e646576696365732e616273656e74 = no registered devices

m6a6f75726e616c2e636c692e646f63746f722e646576696365732e6d6f7265 = { $devices }{ $separator }+{ $extra } more

m6a6f75726e616c2e636c692e646f63746f722e646576696365732e756e617661696c61626c65 = device records unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e6c61796f75742e756e737570706f72746564 = resolved installation root is not a layout install tree

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e7061636b6167652e696e7374616c6c = a package install is replaced in place when it updates, so no second copy is needed

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e726f6f742e756e7265736f6c766564 = could not resolve installation root from install bin directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73697a652e6f766572666c6f77 = installation tree size overflow at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73697a652e756e7265616461626c65 = could not measure installation tree at { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e617661696c61626c65 = { $free_gib } { "GiB" } free on the filesystem holding { $path } ({ $required_gib } { "GiB" } is a second copy of this install plus headroom)

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e696e7370656374 = could not inspect free space at { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e6c6f77 = only { $free_gib } { "GiB" } free on the filesystem holding { $path }; { $required_gib } { "GiB" } is a second copy of this install plus headroom

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e756e7265616461626c65 = free space is not readable at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e616374696f6e2e7265636f7665726564 = no action is needed: recovered records stay beside the originals and are listed here

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e616374696f6e2e72656675736564 = the journal will not start until this clears; each line above says whether it clears after the computer restarts or needs help

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e616374696f6e2e756e7265616461626c65 = check the paths under 'could not inspect'; any recovered records stay beside the originals and are listed here

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v657869746564 = generation { $generation } admission { $admission }: helper { $identity }, exited

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v657363616c61746564 = generation { $generation } admission { $admission }: helper { $identity }, retired (escalated: true)

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v72657469726564 = generation { $generation } admission { $admission }: helper { $identity }, retired

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v756e61636b6e6f776c6564676564 = generation { $generation } admission { $admission }: helper { $identity }, never-acknowledged

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v737061776e = generation { $generation } admission { $admission }: helper { $identity }, spawn-failed

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v756e6964656e746966696564 = generation { $generation } admission { $admission }: helper { $identity }, unidentified

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v72656a6563746564 = generation { $generation } admission { $admission }: helper { $identity }, rejected-and-reaped

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v726575736564 = generation { $generation } admission { $admission }: helper { $identity }, pid-reused-by-another-user

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v657869746564 = generation { $generation } admission { $admission }: service { $identity }, exited

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v657363616c61746564 = generation { $generation } admission { $admission }: service { $identity }, retired (escalated: true)

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v72657469726564 = generation { $generation } admission { $admission }: service { $identity }, retired

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v756e61636b6e6f776c6564676564 = generation { $generation } admission { $admission }: service { $identity }, never-acknowledged

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v737061776e = generation { $generation } admission { $admission }: service { $identity }, spawn-failed

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v756e6964656e746966696564 = generation { $generation } admission { $admission }: service { $identity }, unidentified

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v72656a6563746564 = generation { $generation } admission { $admission }: service { $identity }, rejected-and-reaped

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v726575736564 = generation { $generation } admission { $admission }: service { $identity }, pid-reused-by-another-user

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c65616e = no recorded recovery to report

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c65616e2e6f75747374616e64696e67 = { $i18nCategory ->
    [one] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launch
   *[other] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launches
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c6f736564 = { $i18nCategory ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c6f7365642e726573746172746564 = { $i18nCategory ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e6173696465 = bookkeeping set aside ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e636c6f736564 = runs closed by a later start ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e72656675736564 = starts refused ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e756e7265616461626c65 = could not inspect ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e756e7265736f6c766564 = runs that did not stop cleanly ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e6d6f7265 = ... and { $count } more

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e6f75747374616e64696e67 = { $i18nCategory ->
    [one] the newest run (generation { $generation }) still owes { $launches } launch
   *[other] the newest run (generation { $generation }) still owes { $launches } launches
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e7265667573616c2e72657374617274 = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it clears after the computer restarts

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e7265667573616c2e737570706f7274 = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it will not clear on its own and needs help from support

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e756e7265736f6c766564 = generation { $generation }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e65786563757461626c652e6469726563746f7279 = could not determine executable directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e657865637574696f6e2e6572726f72 = check execution failed: { $kind }

m6a6f75726e616c2e636c692e646f63746f722e657865637574696f6e2e6572726f722e64657461696c = check execution failed: { $kind }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e66616365742e666978 = { $i18nCategory ->
    [one] that segment is missing from the activity lists; check the facet declarations with `{ "solstone journal facet doctor" }` and recent sense runs with `{ "solstone journal talent logs" }`
   *[other] those segments are missing from the activity lists; check the facet declarations with `{ "solstone journal facet doctor" }` and recent sense runs with `{ "solstone journal talent logs" }`
}

m6a6f75726e616c2e636c692e646f63746f722e66616365742e6e6f2e616374697665 = no active segments in the newest days

m6a6f75726e616c2e636c692e646f63746f722e66616365742e726f75746564 = { $i18nCategory ->
    [one] { $routed } of { $active } active segment since { $since } filed under a facet
   *[other] { $routed } of { $active } active segments since { $since } filed under a facet
}

m6a6f75726e616c2e636c692e646f63746f722e66616365742e756e726f75746564 = { $i18nCategory ->
    [one] { $unrouted } of { $active } active segment since { $since } { $i18nCategory-unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
   *[other] { $unrouted } of { $active } active segments since { $since } { $i18nCategory-unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
}

m6a6f75726e616c2e636c692e646f63746f722e68656c70 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "\u000a" }Run { "solstone" } diagnostics.{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "         " }show this help message and exit{ "\u000a" }{ "  " }{ "--verbose" }{ "          " }print every check result{ "\u000a" }{ "  " }{ "--json" }{ "             " }emit JSON instead of text{ "\u000a" }{ "  " }{ "--jsonl" }{ "            " }emit one-JSON-per-line events instead of text{ "\u000a" }{ "  " }{ "--port PORT" }{ "        " }port to probe (default: { "5015" }){ "\u000a" }{ "  " }{ "--readiness" }{ "        " }run the setup readiness battery{ "\u000a" }{ "\u000a" }If '{ "solstone journal doctor" }' is unavailable, build the native doctor from this checkout:{ "\u000a" }{ "cargo run --manifest-path core/Cargo.toml -p solstone-core-doctor --bin solstone-core-doctor" }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e706172746974696f6e = held partition { $partition }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e706172746974696f6e2e73746f70706564 = partition { $partition } was running when the journal stopped; the journal holds it at its next start until its processes are gone

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e656d707479 = record file is empty

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e66696c656e616d65 = invalid partition file name in scope directory; the journal still reads it as a task record

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e696e76616c6964 = cannot parse record: { $error }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e7374726179 = non-record entry in scope directory

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e756e7265616461626c65 = cannot read record: { $error }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e72656d6f7665 = remove { $path }: it holds nothing

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e73636f70652e756e6b6e6f776e = unrecognized scope directory format; the journal still holds any task recorded in it

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e73636f70652e756e7265616461626c65 = cannot list scope directory: { $error }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7374726179 = stray non-directory in task-queue in-flight path

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e756e7265616461626c65 = cannot list in-flight holds directory: { $error }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e636865636b696e67 = search is checking this itself and rebuilds if the damage is confirmed; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e64656c6574696f6e73 = search keeps retrying these removals; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e6572726f7273 = run { "solstone journal indexer --rescan" } and check its errors

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e6661696c65642e66696c6573 = search retries these updates at its daily check; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e6661696c65642e776f726b = search retries this work every hour; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e66756c6c = run { "solstone journal indexer --rescan-full" }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e72656275696c64 = search rebuilds its index from your journal once the damage is confirmed; if it persists, run { "solstone journal indexer --reset --rescan-full" }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e7265736574 = run { "solstone journal indexer --reset --rescan-full" }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e75706772616465 = update { "solstone" } to the newest version

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e7761726e696e6773 = run { "solstone journal indexer --rescan" } and check its warnings

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e616273656e74 = no local journal

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6261636b6c6f672e756e617661696c61626c65 = couldn't fully determine; backlog read failed: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6368726f6e69636c652e656e7472792e756e617661696c61626c65 = couldn't fully determine; failed to read chronicle entry: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6368726f6e69636c652e756e617661696c61626c65 = couldn't fully determine; failed to read chronicle: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e636f6e666967732e756e617661696c61626c65 = couldn't fully determine; failed to read daily configs: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e636f7665726167652e756e617661696c61626c65 = couldn't determine daily unit coverage on { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e646566696e6974696f6e732e756e617661696c61626c65 = couldn't fully determine; couldn't locate the daily processing definitions: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6661636574732e6461792e756e617661696c61626c65 = couldn't fully determine; failed to check active facets for { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6661636574732e756e617661696c61626c65 = couldn't fully determine; failed to read facets: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e7370656374 = open health to see waiting work and any spending confirmation; reprocess a past day to give it priority after today

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e76656e746f72792e666978 = open health to see waiting work and its next step

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e76656e746f72792e756e617661696c61626c65 = whole-journal inspection unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e76656e746f72792e77616974696e67 = whole-journal inspection has not reported yet

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6f7574736964652e6261636b6c6f67 = daily unit outside the recent backlog on { $day }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e726570616972 = re-run { "solstone journal doctor" }; check the health logs if it persists

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e6964656e74697479 = couldn't read daily unit file { $file_name } on day { $day }: record identity does not match its path

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e696e76616c6964 = couldn't fully determine; malformed daily unit file { $file_name } on day { $day }: { $source }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e756e7265616461626c65 = couldn't fully determine; unreadable daily unit file { $file_name } on day { $day }: { $source }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e76657273696f6e = couldn't read daily unit file { $file_name } on day { $day }: unsupported record version { $version }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e6974732e656e7472792e756e617661696c61626c65 = couldn't fully determine; failed to read daily-unit entry for { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e6974732e756e617661696c61626c65 = couldn't fully determine; failed to read daily-units directory for { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e6d6f7665 = move or remove { $path }, then re-run

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e6e6f742e6469726563746f7279 = journal path exists but is not a directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e706172656e742e756e7772697461626c65 = journal dir absent; nearest existing ancestor is not writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e706172656e742e7772697461626c65 = journal dir absent; parent { $path } is writable

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e756e7772697461626c65 = journal dir not writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e7772697461626c65 = journal dir writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e6865616c746879 = the last scan found no unreadable days

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e696e73706563742e666f6c64657273 = check the day folders named above — a name that is not a calendar date is not a day the journal can process

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e7265706169722e66696c6573 = repair the file named above, then run { "solstone journal reprocess <day>" } for each listed day that has already ended

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e726570726f63657373 = run { "solstone journal reprocess <day>" } for each listed day that has already ended

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e73746174732e696e76616c6964 = couldn't check — the statistics aren't readable

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e73746174732e6d697373696e67 = couldn't check — no statistics to read

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e73746174732e7072656461746573 = couldn't check — the statistics predate this version

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e616273656e74 = launchd plist absent

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e617267756d656e74732e6d697373696e67 = plist is missing { "ProgramArguments[0]" }

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e696e76616c6964 = could not parse plist: { "plist::Error" }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e726570616972 = run { "solstone journal service uninstall" }, then run { "solstone journal service install" } separately to reinstall a headless background service

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e7461726765742e657869737473 = launchd plist target exists ({ $path })

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e7461726765742e6d697373696e67 = plist points to missing executable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e66696c652e696e76616c6964 = { $path } is not a file

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e66696c652e6d697373696e67 = { $path } is missing

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e616273656e74 = { "solstone" } is not on { "PATH" }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e6d69736d61746368 = { "PATH" } { "solstone" } resolves to { $resolved }, expected { $expected }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e7265616479 = { "~/.local/bin/solstone" } is on { "PATH" } at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e726570616972 = put { "~/.local/bin" } earlier on { "PATH" }, or run { "solstone journal setup" } to repoint the managed wrapper

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e73796d6c696e6b2e7265616479 = { "~/.local/bin/solstone" } symlinks to { "PATH" } { "solstone" } at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e706174682e7072696f72697479 = put { $install_dir } ahead of { $other_dir } on your { "PATH" }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e7265616479 = { "solstone" } is on { "PATH" } at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e726570616972 = reinstall the journal to restore { "solstone.exe" }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e7465726d696e616c = open a new terminal window and try again; if { "solstone" } is still not found, reinstall the journal

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e777261707065722e696e7374616c6c = run { "solstone journal setup" } to install the managed { "solstone" } wrapper

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7064662e666f756e64 = { $i18nCategory ->
    [one] { $count } raw PDF original without a readable document transcript: { $paths }
   *[other] { $count } raw PDF original(s) without a readable document transcript: { $paths }
}

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7365676d656e742e7064662e6368726f6e69636c652e756e617661696c61626c65 = chronicle directory unavailable

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7365676d656e742e7064662e636c656172 = all raw PDF originals have readable document transcripts

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7365676d656e742e7064662e726570616972 = add a readable { "*_transcript.md" } beside the PDF original, then re-run { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e65786563757461626c652e706172656e74 = executable has no parent

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e6c61796f7574 = could not locate packaged talent roots from executable directory { $directory }{ "\u000a" }walked ancestor { $ancestor }{ "\u000a" }site-packages candidate: { $site }{ "\u000a" }checkout candidate ({ "pyproject.toml" } + { ".git" }): { $checkout }{ "\u000a" }layout share anchors { $bundle }, { $layout }, { $template }: { $install }{ "\u000a" }Apple bundle { "JournalPayload" } candidate: { $embedded }

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e6d697373696e67 = installation root { $root } is missing directory { $path }

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e6e6f6e65 = none

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e746172676574 = { "\u000a" }Cargo target directory { $target } is outside any checkout: a binary built there cannot find the payload, so keep { "CARGO_TARGET_DIR" } inside the checkout

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e756e617661696c61626c65 = could not locate packaged talent roots from executable directory { $directory }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e636f6e66696775726564 = parakeet check failed: installed cache { $installed } does not match configured { $configured }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e646967657374 = parakeet check failed: asset digest mismatch for { $file }: expected { $expected }, found { $found }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e6661696c6564 = parakeet check failed: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e6d697373696e67 = parakeet check failed: asset missing: { $file }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e6d697373696e672e6572726f72 = parakeet check failed: asset missing: { $file }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e73656e74696e656c = parakeet check failed: sentinel not ready at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e73697a65 = parakeet check failed: asset size mismatch for { $file }: expected { $expected }, found { $found }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e756e7265616461626c65 = parakeet check failed: failed to read { $file }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e65786563757461626c65 = { "parakeet-cpp" } check failed: { $name } not executable at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e6d697373696e67 = { "parakeet-cpp" } check failed: { $name } missing at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e6d6f64656c = { "parakeet-cpp" } check failed: model missing at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e70696e = { "parakeet-cpp" } pin lacks { $key }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e706c6174666f726d = { "parakeet-cpp" } is unsupported on { $os }/{ $arch }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e6261636b656e642e6e6f742e73656c6563746564 = configured backend is not { "parakeet-cpp" }; check not applicable

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e62696e6172792e756e617661696c61626c65 = { "parakeet-cpp" } binary cannot start

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e696e7374616c6c = { "parakeet-cpp" } artifacts are not installed — fetch them with: { "solstone journal install-provider parakeet" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e6f66666c696e65 = { "parakeet-server" } not reachable: your journal isn't running

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e6f6e6c696e65 = { "parakeet-cpp" } ready (server + model match the signed package, server reachable)

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e7265616479 = { "parakeet-cpp" } ready (binaries + model installed, server reachable)

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e72756e74696d652e696e7374616c6c = install the system { "OpenMP" } runtime that provides { "libgomp.so.1" }, then rerun { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e72756e74696d652e756e617661696c61626c65 = { "parakeet-cpp" } cannot start: { "OpenMP" } runtime unavailable ({ "libgomp.so.1" })

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e7365727665722e6d697373696e67 = { "parakeet-server" } not reachable: your journal is running, but not { "parakeet-server" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e7374617274 = { "parakeet-server" } is not reachable — start the journal service: { "solstone journal up" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e756e726561636861626c65 = { "parakeet-server" } not reachable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e756e737570706f72746564 = { "parakeet-cpp" } is only supported on linux and windows

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e7068617365 = { "parakeet-server" } not reachable: your journal reports its state as { $phase }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e70686173652e636865636b696e67 = { "parakeet-server" } not reachable: your journal reports its state as checking

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e7265696e7374616c6c = reinstall the journal

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v616273656e74 = { "parakeet-server" } not reachable: nothing is listening

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v756e6b6e6f776e = { "parakeet-server" } not reachable: can't tell whether anything is listening

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v72756e74696d65 = { "parakeet-server" } not reachable: nothing answered

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v74696d656f7574 = { "parakeet-server" } not reachable: took too long to answer

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v7472616e73706f7274 = { "parakeet-server" } not reachable: the connection dropped

m6a6f75726e616c2e636c692e646f63746f722e706174682e726570616972 = fix ownership/permissions of { $path }

m6a6f75726e616c2e636c692e646f63746f722e706c6174666f726d2e756e737570706f72746564 = not supported on { $platform }

m6a6f75726e616c2e636c692e646f63746f722e70726f67726573732e636865636b = { $seconds }s checking { $name }

m6a6f75726e616c2e636c692e646f63746f722e70726f67726573732e68617368 = { $seconds }s verifying { $member }: { $hashed } of { $total } { "MB" }

m6a6f75726e616c2e636c692e646f63746f722e7365617263682e6c6f672e66696e6973686564 = last finished search check could not be read: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7365617263682e6c6f672e756e617661696c61626c65 = search index status unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7365617263682e6c6f672e776f726b = search work list could not be read: { $error }

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6361707065642e726561736f6e = at least 99 Sense dispatch errors since the last successful handler sync; latest reason: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6361707065642e756e6b6e6f776e = at least 99 Sense dispatch errors since the last successful handler sync; latest reason unavailable

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e636c656172 = no Sense dispatch errors since the last successful handler sync

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e636f756e742e696e76616c6964 = Sense status is incomplete: no valid nonnegative dispatch error count

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e636f756e742e6f75742e6f662e72616e6765 = Sense status is incomplete: dispatch error count is outside its supported range

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6572726f72732e726561736f6e = { $i18nCategory ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason: { $reason }
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason: { $reason }
}

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6572726f72732e756e6b6e6f776e = { $i18nCategory ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason unavailable
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason unavailable
}

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v616273656e74 = could not confirm Sense dispatch health: nothing is listening

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v756e6b6e6f776e = could not confirm Sense dispatch health: can't tell whether anything is listening

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v72756e74696d65 = could not confirm Sense dispatch health: nothing answered

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v74696d656f7574 = could not confirm Sense dispatch health: took too long to answer

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v7472616e73706f7274 = could not confirm Sense dispatch health: the connection dropped

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e616273656e74 = no local journal service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e636f6e666c6963742e7761726e696e67 = resolve the macOS supervisor topology warning before changing the journal service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e63726173682e6c697374 = crash-loop: { $services }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e726570616972 = run { "solstone journal setup" } to reinstall the service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7265706169722e666f726365 = run { "solstone journal setup --force" } from this install to refresh the service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7461726765742e616273656e74 = service unit is malformed: no executable target in { $path }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7461726765742e63757272656e74 = service target matches current install: { $raw } -> { $path }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7461726765742e6d69736d61746368 = service target mismatch: { $raw } resolves to { $resolved }, expected { $expected }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e77696e646f77732e63757272656e74 = service target matches current install: { $command }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e77696e646f77732e6d69736d61746368 = service target mismatch: { $mismatch }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e726567697374726174696f6e2e756e7265616461626c65 = couldn't read the journal's service registration — { $reason }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e726573746172742e617474656d707473 = { $i18nCategory ->
    [one] { $name } ({ $attempts } restart attempt)
   *[other] { $name } ({ $attempts } restart attempts)
}

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e6c6f6773 = run { "solstone journal service logs" }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e7069642e696e76616c6964 = probe child PID does not fit a process-group ID

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e726567697374726174696f6e2e6e6f2e726561646572 = no reader for the service registration

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e72657374617274 = run { "solstone journal service restart" }; if it persists, run { "solstone journal service logs" }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e72756e6e696e67 = journal service is running

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e7374617274 = run { "solstone journal service start" }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e73746f70706564 = service installed but not running

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e69742e6661696c6564 = journal service unit is failed

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v616273656e74 = couldn't get a status — nothing is listening

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v756e6b6e6f776e = couldn't get a status — can't tell whether anything is listening

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v72756e74696d65 = couldn't get a status — nothing answered

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v74696d656f7574 = couldn't get a status — took too long to answer

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v7472616e73706f7274 = couldn't get a status — the connection dropped

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e636865636b6f75742e7265717569726564 = router skill state is only available from a source checkout

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6469726563746f726965732e6d697373696e67 = router skill directories are unavailable

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e696e7374616c6c = run { "solstone skills install --project ." }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b2e666f726569676e = { $skill } points elsewhere at { $path }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b2e6d697373696e67 = { $skill } missing at { $path }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b2e7374616c65 = stale router skill link at { $path }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b732e7265616479 = router skills { "solstone" }, journal are installed and current

m6a6f75726e616c2e636c692e646f63746f722e736f757263652e64656661756c74 = default

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e646179732e6d6f7265 = { $days }, and { $more } more

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e666f6c642e6661696c6564 = { $i18nCategory ->
    [one] { $count } day didn't finish processing ({ $days })
   *[other] { $count } day(s) didn't finish processing ({ $days })
}

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e737461746973746963732e72656672657368 = run { "solstone journal journal-stats" }; check the health logs if it persists

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e737461746973746963732e7374616c65 = couldn't check — the statistics are { $hours } hours old

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e756e7265616461626c65 = { $i18nCategory ->
    [one] { $count } day couldn't be scanned ({ $days })
   *[other] { $count } day(s) couldn't be scanned ({ $days })
}

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e756e7265616461626c652e6361757365 = { $i18nCategory ->
    [one] { $count } day couldn't be scanned ({ $days }): { $day }: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): { $day }: { $cause }
}

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e756e7265616461626c652e756e6461746564 = { $i18nCategory ->
    [one] { $count } day couldn't be scanned ({ $days }): that day: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): that day: { $cause }
}

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e696e7374616c6c6174696f6e2e617070 = { "Speakers-analyze" } installation is incomplete ({ $detail }). Repair: reinstall the journal app.

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e696e7374616c6c6174696f6e2e6a6f75726e616c = { "Speakers-analyze" } installation is incomplete ({ $detail }). Repair: reinstall the journal.

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e7265616479 = { "speakers-analyze" } installation ready

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e7265696e7374616c6c2e617070 = reinstall the journal app

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e756e617661696c61626c65 = { "speakers-analyze" } installation unavailable

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e6572726f72 = ERROR

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e6661696c = FAIL

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e6f6b = OK

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e736b6970 = SKIP

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e7761726e = WARN

m6a6f75726e616c2e636c692e646f63746f722e73746f726167652e696e7370656374696f6e2e6c6f67 = storage privacy: { $error }

m6a6f75726e616c2e636c692e646f63746f722e73746f726167652e707269766163792e63686f696365 = other accounts on this computer may be able to read your journal. its folder could not be made private on this drive. you can keep it on this drive or move it to a drive that keeps folders private. to keep it on this drive, choose that on your journal's health page.

m6a6f75726e616c2e636c692e646f63746f722e73756d6d617279 = { $i18nCategory ->
    [one] doctor: { $total } check, { $failed } failed, { $warnings } { $i18nCategory-warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $i18nCategory-error_quantity ->
        [one] error
       *[other] errors
    }
   *[other] doctor: { $total } checks, { $failed } failed, { $warnings } { $i18nCategory-warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $i18nCategory-error_quantity ->
        [one] error
       *[other] errors
    }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e636c656172 = no macOS supervisor conflict (native check: foreign-launcher scan only; no competing KeepAlive launcher found)

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e726570616972 = { $i18nCategory ->
    [one] remove the foreign launcher targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
   *[other] remove foreign launchers targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e7265706f7274 = { $i18nCategory ->
    [one] macOS supervisor conflict: { $count } foreign { "KeepAlive" } launcher targets { "/Applications/solstone.app" }
   *[other] macOS supervisor conflict: { $count } foreign { "KeepAlive" } launchers target { "/Applications/solstone.app" }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e7265736f6c7665 = { $i18nCategory ->
    [one] resolve the macOS supervisor conflict first: remove the foreign launcher targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
   *[other] resolve the macOS supervisor conflict first: remove foreign launchers targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e756e737570706f72746564 = not supported on windows

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6469726563746f72792e7265717569726564 = flat-directory descendant is not a directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e6368616e676564 = entry changed during observation: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e696e76616c6964 = invalid flat-directory entry { $entry }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e7265706c61636564 = changed or disappeared during inspection

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e756e73616665 = Installation: needs attention{ "\u000a" }your journal contains an item that can't be checked safely.{ "\u000a" }move the item named under details out of your journal's `{ "health/sync" }` folder, then try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }path: { $path }{ "\u000a" }type: { $entry_type }{ "\u000a" }reason: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e756e736166652e6279746573 = Installation: needs attention{ "\u000a" }your journal contains an item that can't be checked safely.{ "\u000a" }move the item named under details out of your journal's `{ "health/sync" }` folder, then try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }path: { $path }{ "\u000a" }type: { $entry_type }{ "\u000a" }reason: { $reason }{ "\u000a" }name bytes: { $bytes }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e76616e6973686564 = flat-directory entry vanished while listing: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e66696c652e7265717569726564 = flat-directory entry is not a regular file: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e666f6c6465722e66756c6c = Installation: needs attention{ "\u000a" }your journal has too many items to check safely.{ "\u000a" }review the items in your journal's `{ "health/sync" }` folder. move only items you know do not belong there until 256 or fewer remain, then try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }folder: { $folder }{ "\u000a" }items found: more than 256{ "\u000a" }maximum checked: 256

m6a6f75726e616c2e636c692e646f63746f722e73796e632e666f6c6465722e756e73616665 = Installation: needs attention{ "\u000a" }part of your journal can't be checked safely.{ "\u000a" }check the path named under details. it needs to be an ordinary folder inside your journal that you can open before you try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }path: { $path }{ "\u000a" }reason: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e686561727462656174 = Installation: needs attention{ "\u000a" }a recent heartbeat from another run is present.{ "\u000a" }wait a moment, then try again.

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6964656e746974792e6368616e676564 = flat-directory identity changed: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e696f = { $operation } failed for { $path }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e626c6f636b646576696365 = block device

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e636861726163746572646576696365 = character device

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e6469726563746f7279 = directory

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e6669666f = named pipe

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e6f74686572 = other filesystem object

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e726567756c617266696c65 = regular file

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e736f636b6574 = Unix socket

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e73796d6c696e6b = symbolic link

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6c6f63616c = this computer only ({ $host })

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706174682e696e76616c6964 = invalid flat-directory path { $path }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706565722e6c6567616379 = this computer only ({ $host }){ "\u000a" }  last other writer: { $writer } (legacy machine { $prefix }...)

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706565722e756e6b6e6f776e = this computer only ({ $host }){ "\u000a" }  last other writer: { $writer } (unidentified heartbeat)

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706565722e777269746572 = this computer only ({ $host }){ "\u000a" }  last other writer: { $writer } (writer { $prefix }... run { $run })

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e6469736170706561726564 = entry disappeared during observation

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e6e6f6e726567756c6172 = not a regular file ({ $kind })

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e6f76657273697a6564 = regular file is { $size } bytes, limit is { $limit }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e72656164 = could not read observed file

m6a6f75726e616c2e636c692e646f63746f722e73796e632e73697a65 = flat-directory entry exceeds observed-read limit: { $path } is { $kind }, { $size } bytes exceeds { $limit }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e73796d6c696e6b = flat-directory descendant is a symlink: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e756e737570706f72746564 = Installation: needs attention{ "\u000a" }sync rescan capability is unavailable on this platform.

m6a6f75726e616c2e636c692e646f63746f722e73796e632e756e76657269666961626c65 = Installation: needs attention{ "\u000a" }startup status couldn't be verified.{ "\u000a" }wait a moment, then try again.

m6a6f75726e616c2e636c692e646f63746f722e73796e632e77616974696e67 = Installation: waiting{ "\u000a" }a recent heartbeat from another run was found.{ "\u000a" }the { "solstone" } app is waiting while it checks whether that activity continues. it will check again shortly.

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e706163652e61626f75742e636170 = a job is running long; it will be stopped automatically if it passes its cap — no action needed unless it persists

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e706163652e6865616c746879 = tasks on pace

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e706163652e7374617475732e756e617661696c61626c65 = supervisor status unavailable

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e71756575652e686f6c64732e616273656e74 = no in-flight or held task partition records present

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e71756575652e686f6c64732e636c656172 = all task partition records are clean

m6a6f75726e616c2e636c692e646f63746f722e7461736b732e6475726174696f6e = { $task } ({ $seconds }s of { $limit }s cap)

m6a6f75726e616c2e636c692e646f63746f722e7461736b732e6c6f6e67 = running long: { $tasks }

m6a6f75726e616c2e636c692e646f63746f722e7472616e736372697074696f6e2e756e726574727961626c65 = { $i18nCategory ->
    [one] { $count } recording the journal will not retry on its own ({ "handler" }: { $handler }, { "reason_code" }: { $reason_code }): { $identities }
   *[other] { $count } recordings the journal will not retry on its own ({ "handler" }: { $handler }, { "reason_code" }: { $reason_code }): { $identities }
}

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e636c656172 = 0 recordings the journal will not retry on its own (handler: { $handler }, { "reason_code" }: { $reason_code })

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e646179732e756e7265616461626c65 = failed to read chronicle days: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e6469726563746f72792e756e7265616461626c65 = cannot read segment dir { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e656e7472792e756e7265616461626c65 = cannot read segment entry in { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e68616e646c65722e6d697373696e67 = processing record in { $path } missing handler field

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e6964656e746974792e696e76616c6964 = invalid segment identity in { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e6f70656e = cannot open { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e72656164 = cannot read { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e656e636f64696e67 = first row in { $path } is not valid utf-8: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e6a736f6e = first row in { $path } is not valid json: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e6c61726765 = first row in { $path } exceeds byte limit

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e756e617661696c61626c65 = first row in { $path } cannot be established or exceeds byte limit

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e7363616e2e6661696c6564 = could not determine how many recordings the journal will not retry on its own ({ $reason })

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e7365676d656e74732e756e7265616461626c65 = failed to list segments for day { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e75736167652e617267756d656e74 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "solstone journal doctor" }: error: unexpected argument

m6a6f75726e616c2e636c692e646f63746f722e75736167652e666f726d617473 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "solstone journal doctor" }: error: { "--json" } and { "--jsonl" } are mutually exclusive

m6a6f75726e616c2e636c692e646f63746f722e75736167652e706f7274 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "solstone journal doctor" }: error: { "--port" } requires an integer

m6a6f75726e616c2e636c692e646f63746f722e7661642e636f6e7472616374 = { "VAD" } helper ran but did not report closed-stdin usage contract (exit { $exit_code }): { $stderr }

m6a6f75726e616c2e636c692e646f63746f722e7661642e65786563757461626c65 = could not determine current executable for { "VAD" } helper: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7661642e6c61796f7574 = { "VAD" } helper lookup is outside the running signed app payload

m6a6f75726e616c2e636c692e646f63746f722e7661642e6c6f61646572 = { "VAD" } helper could not load { $library }

m6a6f75726e616c2e636c692e646f63746f722e7661642e6d697373696e67 = { "VAD" } helper binary is missing: { $path }

m6a6f75726e616c2e636c692e646f63746f722e7661642e706172656e74 = current executable has no parent directory for { "VAD" } helper lookup

m6a6f75726e616c2e636c692e646f63746f722e7661642e70726f62652e6c6f67 = { "VAD" } helper probe failed: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265616479 = { "VAD" } helper launchable (closed-stdin usage contract)

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e696e7374616c6c = reinstall the journal, then rerun { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e696e7374616c6c2e617070 = reinstall the journal app, then rerun { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e72657374617274 = restart the journal, then rerun { "solstone journal doctor" }; if it still times out, reinstall the journal

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e726573746172742e617070 = restart the journal app, then rerun { "solstone journal doctor" }; if it still times out, reinstall the journal app

m6a6f75726e616c2e636c692e646f63746f722e7661642e737061776e = { "VAD" } helper could not be executed: { $cause }

m6a6f75726e616c2e636c692e646f63746f722e7661642e74696d656f7574 = { "VAD" } helper did not exit within { $timeout }

m6a6f75726e616c2e636c692e646f63746f722e7661642e756e7265736f6c766564 = could not resolve the { "VAD" } helper path: { $cause }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e68656c706572 = { "ONNX" } helper is not the declared member of the signed app payload

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e6d656d626572 = signed { "ONNX" } app payload refuses { $member }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e706c6174666f726d = { "Windows" } { "ONNX" } package verification requires a { "Windows" } runtime

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e766572696669636174696f6e = could not verify the signed { "ONNX" } app payload: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e65786563757461626c65 = could not determine the running journal executable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e6c61796f7574 = running journal executable is not in the package bin directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e6d656d626572 = signed { "Parakeet" } app payload refuses { $member }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e706172656e74 = running journal executable has no containing directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e706c6174666f726d = Windows { "Parakeet" } package verification requires a Windows runtime

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e726f6f74 = package bin directory has no package root: { $path }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e766572696669636174696f6e = could not verify the signed { "Parakeet" } app payload: { $error }

m6a6f75726e616c2e636c692e73747265616d732e617267756d656e7473 = usage: { "solstone journal streams [-h] [--rebuild] [-v] [-d] [name]" }{ "\u000a" }{ "solstone journal streams" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e73747265616d732e656d707479 = No streams found

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e646179 = Last Day

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e6e616d65 = Name

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e7365676d656e74 = Last Segment

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e73657175656e6365 = Seq

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e74797065 = Type

m6a6f75726e616c2e636c692e73747265616d732e68656c70 = usage: { "solstone journal streams [-h] [--rebuild] [-v] [-d] [name]" }{ "\u000a\u000a" }Inspect and manage stream identity{ "\u000a\u000a" }positional arguments:{ "\u000a" }{ "  " }{ "name" }{ "           " }Stream name to inspect (omit to list all streams){ "\u000a\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "     " }show this help message and exit{ "\u000a" }{ "  " }{ "--rebuild" }{ "      " }Reconstruct stream state from per-segment markers{ "\u000a" }{ "  " }{ "-v, --verbose" }{ "  " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "    " }Enable debug logging

m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6d616c666f726d6564 = Could not read stream { $path }: malformed record

m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6e6f742e666f756e64 = Stream not found: { $name }

m6a6f75726e616c2e636c692e73747265616d732e6c6973742e6572726f72 = Could not list streams: { $error }

m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e73657175656e6365 = unreadable marker { $path }: invalid sequence

m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e756e7265616461626c65 = unreadable marker { $path }: could not read marker

m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e656d707479 = { $i18nCategory ->
    [one] No streams found ({ $segments } segment scanned)
   *[other] No streams found ({ $segments } segments scanned)
}

m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e726573756c74 = { $i18nCategory ->
    [one] Rebuilt { $streams } stream from { $i18nCategory-segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
   *[other] Rebuilt { $streams } stream(s) from { $i18nCategory-segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
}

m6a6f75726e616c2e636c692e73747265616d732e7265636f72642e6d616c666f726d6564 = Could not read stream record { $path }: malformed record

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6168656164 = { $stream }: stream state sequence is ahead of markers; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6c6f636b6564 = { $stream }: could not lock { $path }; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d616c666f726d6564 = { $stream }: malformed stream record at { $path }; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d697373696e67 = { $stream }: no stream record at { $path }; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e77726974652e6661696c6564 = { $stream }: write failed for { $path }: atomic publication failed

m6a6f75726e616c2e636c692e73747265616d732e7365676d656e74732e6572726f72 = Could not list segments: { $error }

m6a6f75726e616c2e636c692e73747265616d732e73757065727669736f72 = journal isn't running. start it with '{ "solstone journal up" }' and retry.

