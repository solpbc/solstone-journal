push.test.title = [test] solstone
push.test.body = [test] test notification from your journal.
push.proof.count = { $count ->
    [one] [test] { $count } item is ready.
   *[other] [test] { $count } items are ready.
    }
push.proof.state = { $state ->
    [ready] [test] ready for the test.
   *[waiting] [test] waiting for the test.
    }
push.proof.new = [test] a new test message is ready.
