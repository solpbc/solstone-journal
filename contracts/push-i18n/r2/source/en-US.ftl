push.test.title = solstone
push.test.body = test notification from your journal.
push.proof.count = { $count ->
    [one] { $count } item is ready.
   *[other] { $count } items are ready.
    }
push.proof.state = { $state ->
    [ready] ready for the test.
   *[waiting] waiting for the test.
    }
push.proof.new = a new test message is ready.
