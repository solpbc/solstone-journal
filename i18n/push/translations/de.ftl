push.proof.count = { $count ->
    [one] { $count } Element ist bereit.
   *[other] { $count } Elemente sind bereit.
}

push.proof.new = eine neue Testmitteilung ist bereit.

push.proof.state = { $state ->
    [ready] bereit für den Test.
   *[waiting] wartet auf den Test.
}

push.test.body = Testbenachrichtigung von deinem Journal.

push.test.title = solstone
